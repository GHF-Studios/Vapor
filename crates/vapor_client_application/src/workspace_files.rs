//! Guarded filesystem operations shared by Studio's service and authoring tools.

use std::{
    fs,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_TEXT_FILE_BYTES: u64 = 4 * 1024 * 1024;

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
    let file = fs::OpenOptions::new()
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
    drop(file);
    Ok(relative.to_path_buf())
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
}
