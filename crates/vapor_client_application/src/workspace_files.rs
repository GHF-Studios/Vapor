//! Guarded filesystem operations shared by Studio's service and authoring tools.

use std::{
    fs,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use vapor_client_domain::CanonicalRustModuleKind;

const MAX_TEXT_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Files updated when creating a convention-owned Rust module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalRustModuleCreated {
    pub module_path: PathBuf,
    pub module_contents: String,
    pub module_fingerprint: String,
    pub parent_path: PathBuf,
    pub parent_contents: String,
    pub parent_fingerprint: String,
}

/// Lists regular files below a workspace, excluding generated and VCS trees.
pub fn discover_workspace_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let root = fs::canonicalize(root).map_err(|error| format!("{}: {error}", root.display()))?;
    let mut pending = vec![root.clone()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory)
            .map_err(|error| format!("{}: {error}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(
                name.as_ref(),
                ".git"
                    | ".hg"
                    | ".svn"
                    | ".cache"
                    | "target"
                    | "node_modules"
                    | "vendor"
                    | "output"
            ) {
                continue;
            }
            let file_type = entry.file_type().map_err(|error| error.to_string())?;
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                let relative = entry
                    .path()
                    .strip_prefix(&root)
                    .map_err(|error| error.to_string())?
                    .to_path_buf();
                files.push(relative);
                if files.len() > 10_000 {
                    return Err(
                        "workspace contains more than 10,000 files; narrow the source root"
                            .to_owned(),
                    );
                }
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Reads a UTF-8 workspace text file and returns its optimistic concurrency fingerprint.
pub fn read_workspace_file_with_fingerprint(
    root: &Path,
    relative: &Path,
) -> Result<(String, String), String> {
    let path = safe_workspace_file(root, relative)?;
    let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_TEXT_FILE_BYTES {
        return Err(format!(
            "files larger than {} MiB cannot be opened in the workspace text editor",
            MAX_TEXT_FILE_BYTES / 1024 / 1024
        ));
    }
    let contents = fs::read_to_string(&path).map_err(|error| {
        format!(
            "{} is not a readable UTF-8 text file: {error}",
            relative.display()
        )
    })?;
    let fingerprint = workspace_file_fingerprint(&contents);
    Ok((contents, fingerprint))
}

/// Reads a UTF-8 workspace text file.
pub fn read_workspace_file(root: &Path, relative: &Path) -> Result<String, String> {
    read_workspace_file_with_fingerprint(root, relative).map(|(contents, _)| contents)
}

/// Returns a stable, bounded fingerprint for optimistic file-save checks.
pub fn workspace_file_fingerprint(contents: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in contents.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}:{}", contents.len())
}

/// Saves a workspace file only if its current contents still match the opened version.
pub fn write_workspace_file_if_fingerprint(
    root: &Path,
    relative: &Path,
    expected_fingerprint: &str,
    contents: &str,
) -> Result<String, String> {
    let path = safe_workspace_file(root, relative)?;
    let _lock = WorkspaceFileEditLock::acquire(workspace_edit_lock_path(&path))?;
    let metadata =
        fs::metadata(&path).map_err(|error| format!("{}: {error}", relative.display()))?;
    if metadata.len() > MAX_TEXT_FILE_BYTES {
        return Err(format!(
            "files larger than {} MiB cannot be saved through the workspace text editor",
            MAX_TEXT_FILE_BYTES / 1024 / 1024
        ));
    }
    let current =
        fs::read_to_string(&path).map_err(|error| format!("{}: {error}", relative.display()))?;
    if workspace_file_fingerprint(&current) != expected_fingerprint {
        return Err(format!(
            "{} changed on disk after it was opened; reload it before saving to avoid overwriting those edits",
            relative.display()
        ));
    }

    let temporary = sibling_edit_path(&path, "tmp");
    let backup = sibling_edit_path(&path, "backup");
    let write_result = (|| -> Result<(), String> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("could not prepare {}: {error}", relative.display()))?;
        use std::io::Write;
        file.write_all(contents.as_bytes())
            .map_err(|error| format!("could not write {}: {error}", relative.display()))?;
        fs::set_permissions(&temporary, metadata.permissions()).map_err(|error| {
            format!(
                "could not preserve permissions for {}: {error}",
                relative.display()
            )
        })?;
        file.sync_all()
            .map_err(|error| format!("could not sync {}: {error}", relative.display()))?;
        let latest = fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", relative.display()))?;
        if workspace_file_fingerprint(&latest) != expected_fingerprint {
            return Err(format!(
                "{} changed on disk while the save was being prepared; reload before saving",
                relative.display()
            ));
        }
        fs::rename(&path, &backup).map_err(|error| {
            format!(
                "could not preserve original {}: {error}",
                relative.display()
            )
        })?;
        if let Err(error) = fs::rename(&temporary, &path) {
            return Err(match fs::rename(&backup, &path) {
                Ok(()) => format!(
                    "could not replace {}: {error}; original restored",
                    relative.display()
                ),
                Err(restore) => format!(
                    "could not replace {}: {error}; restoring original also failed: {restore}",
                    relative.display()
                ),
            });
        }
        let _ = fs::remove_file(&backup);
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result?;
    Ok(workspace_file_fingerprint(contents))
}

/// Saves a workspace file only if it still contains the expected text.
pub fn write_workspace_file(
    root: &Path,
    relative: &Path,
    expected_contents: &str,
    contents: &str,
) -> Result<(), String> {
    write_workspace_file_if_fingerprint(
        root,
        relative,
        &workspace_file_fingerprint(expected_contents),
        contents,
    )
    .map(|_| ())
}

/// Creates a new workspace file without following symlinks or overwriting content.
pub fn create_new_workspace_file(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    create_new_workspace_file_with_contents(root, relative, "")
}

fn create_new_workspace_file_with_contents(
    root: &Path,
    relative: &Path,
    contents: &str,
) -> Result<PathBuf, String> {
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(
            "file path must be relative and cannot contain '.', '..', or root components"
                .to_owned(),
        );
    }
    let root = fs::canonicalize(root).map_err(|error| format!("{}: {error}", root.display()))?;
    let requested = root.join(relative);
    let relative_parent = relative
        .parent()
        .ok_or_else(|| "file path has no parent directory".to_owned())?;
    let mut parent = root.clone();
    let mut created_directories = Vec::new();
    for component in relative_parent.components() {
        let Component::Normal(name) = component else {
            return Err("file parent path must stay inside the workspace".to_owned());
        };
        parent.push(name);
        match fs::symlink_metadata(&parent) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                cleanup_created_directories(&created_directories);
                return Err(format!(
                    "file parent must use regular workspace directories: {}",
                    parent.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Err(error) = fs::create_dir(&parent) {
                    cleanup_created_directories(&created_directories);
                    return Err(format!(
                        "could not create folder {}: {error}",
                        parent.display()
                    ));
                }
                created_directories.push(parent.clone());
            }
            Err(error) => {
                cleanup_created_directories(&created_directories);
                return Err(format!("{}: {error}", parent.display()));
            }
        }
        let canonical_parent = match fs::canonicalize(&parent) {
            Ok(path) => path,
            Err(error) => {
                cleanup_created_directories(&created_directories);
                return Err(format!("{}: {error}", parent.display()));
            }
        };
        if !canonical_parent.starts_with(&root) {
            cleanup_created_directories(&created_directories);
            return Err("file path must stay inside the workspace".to_owned());
        }
        parent = canonical_parent;
    }
    let Some(name) = requested.file_name() else {
        cleanup_created_directories(&created_directories);
        return Err("file path must include a filename".to_owned());
    };
    let target = parent.join(name);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(|error| {
            cleanup_created_directories(&created_directories);
            format!(
                "could not create {} without overwriting: {error}",
                relative.display()
            )
        })?;
    use std::io::Write;
    if let Err(error) = file
        .write_all(contents.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(&target);
        cleanup_created_directories(&created_directories);
        return Err(format!(
            "could not initialize {}: {error}",
            relative.display()
        ));
    }
    Ok(relative.to_path_buf())
}

/// Creates one canonical Rust module and registers it from its existing parent.
///
/// The new module is created first, then the parent is saved with optimistic
/// concurrency protection. A failed parent update removes the new module, so
/// callers never receive success for a half-applied module operation.
pub fn create_canonical_rust_module(
    root: &Path,
    parent_relative: &Path,
    expected_parent_fingerprint: &str,
    module_kind: CanonicalRustModuleKind,
) -> Result<CanonicalRustModuleCreated, String> {
    let module_name = module_kind.name();
    let root = fs::canonicalize(root).map_err(|error| format!("{}: {error}", root.display()))?;
    let parent_path = safe_workspace_file(&root, parent_relative)?;
    if parent_path
        .extension()
        .is_none_or(|extension| extension != "rs")
    {
        return Err("A Rust module must be created from a Rust source file.".to_owned());
    }
    let crate_source = parent_path
        .ancestors()
        .find(|ancestor| ancestor.join("Cargo.toml").is_file())
        .and_then(|crate_root| parent_path.strip_prefix(crate_root).ok())
        .is_some_and(|relative| {
            relative
                .components()
                .next()
                .is_some_and(|component| component.as_os_str() == "src")
        });
    if !crate_source {
        return Err(
            "Canonical modules can only be created under a Cargo crate's `src` directory."
                .to_owned(),
        );
    }
    let parent_contents = fs::read_to_string(&parent_path)
        .map_err(|error| format!("{}: {error}", parent_relative.display()))?;
    if workspace_file_fingerprint(&parent_contents) != expected_parent_fingerprint {
        return Err(format!(
            "{} changed on disk after it was opened; reload it before creating a module",
            parent_relative.display()
        ));
    }
    let parsed = syn::parse_file(&parent_contents).map_err(|_| {
        "Fix syntax errors in the parent module before creating a child.".to_owned()
    })?;
    if parsed
        .items
        .iter()
        .any(|item| matches!(item, syn::Item::Mod(item) if item.ident == module_name))
    {
        return Err(format!(
            "The parent module already declares `{module_name}`."
        ));
    }
    let parent_name = parent_path.file_name().and_then(|name| name.to_str());
    let module_directory = match parent_name {
        Some("lib.rs" | "main.rs" | "mod.rs") => parent_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| "The parent module has no containing directory.".to_owned())?,
        Some(_) => parent_path
            .parent()
            .ok_or_else(|| "The parent module has no containing directory.".to_owned())?
            .join(parent_path.file_stem().unwrap_or_default()),
        None => return Err("The parent module filename must be valid UTF-8.".to_owned()),
    };
    let module_absolute = module_directory.join(format!("{module_name}.rs"));
    let module_relative = module_absolute
        .strip_prefix(&root)
        .map_err(|_| "The new module would be outside the active workspace.".to_owned())?
        .to_path_buf();
    let mut updated_parent = parent_contents.clone();
    if !updated_parent.is_empty() && !updated_parent.ends_with('\n') {
        updated_parent.push('\n');
    }
    if !updated_parent.is_empty() && !updated_parent.ends_with("\n\n") {
        updated_parent.push('\n');
    }
    updated_parent.push_str(&format!("pub mod {module_name};\n"));
    let module_documentation = module_kind.documentation();
    create_new_workspace_file_with_contents(&root, &module_relative, module_documentation)?;
    let parent_relative = parent_path
        .strip_prefix(&root)
        .map_err(|_| "The parent module is outside the active workspace.".to_owned())?;
    let parent_fingerprint = match write_workspace_file_if_fingerprint(
        &root,
        parent_relative,
        expected_parent_fingerprint,
        &updated_parent,
    ) {
        Ok(fingerprint) => fingerprint,
        Err(error) => {
            let _ = fs::remove_file(&module_absolute);
            return Err(error);
        }
    };
    Ok(CanonicalRustModuleCreated {
        module_path: module_relative,
        module_contents: module_documentation.to_owned(),
        module_fingerprint: workspace_file_fingerprint(module_documentation),
        parent_path: parent_relative.to_path_buf(),
        parent_contents: updated_parent,
        parent_fingerprint,
    })
}

fn safe_workspace_file(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    let root = fs::canonicalize(root).map_err(|error| format!("{}: {error}", root.display()))?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("selected path must be relative and stay inside the workspace".to_owned());
    }
    let path = fs::canonicalize(root.join(relative))
        .map_err(|error| format!("{}: {error}", relative.display()))?;
    if !path.starts_with(&root) || !path.is_file() {
        return Err("selected path is not a regular file inside the workspace".to_owned());
    }
    Ok(path)
}

fn workspace_edit_lock_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("workspace-file"))
        .to_string_lossy();
    path.with_file_name(format!(".{name}.studio-edit.lock"))
}

fn sibling_edit_path(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("workspace-file"))
        .to_string_lossy();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    path.with_file_name(format!(
        ".{name}.{}.{}.studio-{suffix}",
        std::process::id(),
        nonce
    ))
}

struct WorkspaceFileEditLock {
    path: PathBuf,
    file: Option<fs::File>,
}

impl WorkspaceFileEditLock {
    fn acquire(path: PathBuf) -> Result<Self, String> {
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| {
                format!(
                    "cannot acquire workspace file edit lock '{}': {error}",
                    path.display()
                )
            })?;
        Ok(Self {
            path,
            file: Some(file),
        })
    }
}

impl Drop for WorkspaceFileEditLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

fn cleanup_created_directories(directories: &[PathBuf]) {
    for directory in directories.iter().rev() {
        let _ = fs::remove_dir(directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestWorkspace(PathBuf);

    impl TestWorkspace {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "vapor-studio-workspace-files-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock follows epoch")
                    .as_nanos()
            ));
            fs::create_dir_all(&path).expect("create fixture workspace");
            Self(path)
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn guarded_text_saves_detect_external_edits_and_return_the_new_fingerprint() {
        let workspace = TestWorkspace::new();
        fs::write(workspace.0.join("Vapor.toml"), "before").expect("write initial file");
        let (original, fingerprint) =
            read_workspace_file_with_fingerprint(&workspace.0, Path::new("Vapor.toml"))
                .expect("read text file");
        assert_eq!(original, "before");

        fs::write(workspace.0.join("Vapor.toml"), "external").expect("external edit");
        let error = write_workspace_file_if_fingerprint(
            &workspace.0,
            Path::new("Vapor.toml"),
            &fingerprint,
            "mine",
        )
        .expect_err("external edits must not be overwritten");
        assert!(error.contains("changed on disk"));
        assert_eq!(
            fs::read_to_string(workspace.0.join("Vapor.toml")).unwrap(),
            "external"
        );

        let external_fingerprint = workspace_file_fingerprint("external");
        let saved_fingerprint = write_workspace_file_if_fingerprint(
            &workspace.0,
            Path::new("Vapor.toml"),
            &external_fingerprint,
            "mine",
        )
        .expect("save against current fingerprint");
        assert_eq!(saved_fingerprint, workspace_file_fingerprint("mine"));
        assert_eq!(
            fs::read_to_string(workspace.0.join("Vapor.toml")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn file_operations_reject_parent_traversal_and_never_overwrite_new_files() {
        let workspace = TestWorkspace::new();
        let error = create_new_workspace_file(&workspace.0, Path::new("../escape.txt"))
            .expect_err("parent traversal is rejected");
        assert!(error.contains("relative"));

        create_new_workspace_file(&workspace.0, Path::new("nested/new.txt"))
            .expect("create nested text file");
        let error = create_new_workspace_file(&workspace.0, Path::new("nested/new.txt"))
            .expect_err("existing file must not be overwritten");
        assert!(error.contains("without overwriting"));
        fs::create_dir_all(workspace.0.join("target")).expect("create generated folder");
        fs::write(workspace.0.join("target/generated.txt"), "generated")
            .expect("write generated file");
        assert_eq!(
            discover_workspace_files(&workspace.0).expect("list workspace"),
            vec![PathBuf::from("nested/new.txt")]
        );
    }

    #[test]
    fn creates_canonical_rust_module_and_updates_parent_with_fingerprint_guard() {
        let workspace = TestWorkspace::new();
        fs::create_dir_all(workspace.0.join("crate/src")).expect("create crate source");
        fs::write(
            workspace.0.join("crate/Cargo.toml"),
            "[package]\nname='sample'\n",
        )
        .expect("write manifest");
        let parent = Path::new("crate/src/lib.rs");
        let original = "#![allow(dead_code)]\n\npub fn ready() {}\n";
        fs::write(workspace.0.join(parent), original).expect("write crate root");
        let expected = workspace_file_fingerprint(original);

        let created = create_canonical_rust_module(
            &workspace.0,
            parent,
            &expected,
            CanonicalRustModuleKind::Components,
        )
        .expect("create canonical module");

        assert_eq!(created.module_path, Path::new("crate/src/components.rs"));
        assert!(workspace.0.join(&created.module_path).is_file());
        assert_eq!(
            fs::read_to_string(workspace.0.join(parent)).unwrap(),
            "#![allow(dead_code)]\n\npub fn ready() {}\n\npub mod components;\n"
        );
        assert_eq!(
            created.parent_fingerprint,
            workspace_file_fingerprint(&created.parent_contents)
        );
    }

    #[test]
    fn canonical_module_creation_rejects_duplicate_or_stale_parent_without_writing() {
        let workspace = TestWorkspace::new();
        fs::create_dir_all(workspace.0.join("crate/src")).expect("create crate source");
        fs::write(
            workspace.0.join("crate/Cargo.toml"),
            "[package]\nname='sample'\n",
        )
        .expect("write manifest");
        let parent = Path::new("crate/src/lib.rs");
        let source = "pub mod components;\n";
        fs::write(workspace.0.join(parent), source).expect("write crate root");

        let duplicate = create_canonical_rust_module(
            &workspace.0,
            parent,
            &workspace_file_fingerprint(source),
            CanonicalRustModuleKind::Components,
        )
        .expect_err("duplicate module declaration must be rejected");
        assert!(duplicate.contains("already declares"));
        assert!(!workspace.0.join("crate/src/components.rs").exists());

        fs::write(workspace.0.join(parent), "pub mod changed;\n").expect("external edit");
        let stale = create_canonical_rust_module(
            &workspace.0,
            parent,
            &workspace_file_fingerprint(source),
            CanonicalRustModuleKind::Types,
        )
        .expect_err("stale parent must be rejected");
        assert!(stale.contains("changed on disk"));
        assert!(!workspace.0.join("crate/src/types.rs").exists());
    }

    #[test]
    fn canonical_module_creation_uses_rust_nested_module_paths_and_rejects_bad_sources() {
        let workspace = TestWorkspace::new();
        fs::create_dir_all(workspace.0.join("crate/src")).expect("create crate source");
        fs::write(
            workspace.0.join("crate/Cargo.toml"),
            "[package]\nname='sample'\n",
        )
        .expect("write manifest");
        let parent = Path::new("crate/src/game.rs");
        let source = "pub fn tick() {}\n";
        fs::write(workspace.0.join(parent), source).expect("write parent module");
        let created = create_canonical_rust_module(
            &workspace.0,
            parent,
            &workspace_file_fingerprint(source),
            CanonicalRustModuleKind::Systems,
        )
        .expect("create nested canonical module");
        assert_eq!(created.module_path, Path::new("crate/src/game/systems.rs"));
        assert!(
            fs::read_to_string(workspace.0.join(&created.module_path))
                .unwrap()
                .starts_with("//! Bevy systems")
        );

        fs::write(workspace.0.join(parent), "pub fn (\n").expect("write invalid Rust source");
        let invalid = create_canonical_rust_module(
            &workspace.0,
            parent,
            &workspace_file_fingerprint("pub fn (\n"),
            CanonicalRustModuleKind::Components,
        )
        .expect_err("syntax errors in the parent are rejected");
        assert!(invalid.contains("Fix syntax errors"));
        assert!(!workspace.0.join("crate/src/game/components.rs").exists());
    }
}
