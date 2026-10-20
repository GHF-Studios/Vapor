//! Build and install Rustdoc for source-root Cargo workspaces.

use crate::{
    discovery::{EnvironmentPaths, ensure_contained},
    managed_cargo::ApplicationCargo,
    workspace::WorkspaceManifest,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Build Rustdoc for every workspace declared by the source manifest.
pub fn build(paths: &EnvironmentPaths, manifest: &WorkspaceManifest) -> Result<PathBuf, String> {
    let cargo = ApplicationCargo::discover(paths)?;
    let installation_root = paths.installation().root();
    let docs_root = installation_root.join("docs");
    ensure_contained(paths.installation().root(), &docs_root)?;
    let staged_docs = TemporaryDirectory::create(installation_root, "docs-staging")?;
    let staged_root = &staged_docs.path;
    let target = documentation_target_root(paths.installation().root());
    ensure_contained(paths.installation().root(), &target)?;
    fs::create_dir_all(&target).map_err(|error| error.to_string())?;

    for section in manifest
        .cargo_projects()
        .iter()
        .filter(|project| project.documentation())
    {
        let cargo_manifest = paths.source().root().join(section.manifest());
        let generated_docs = target.join("doc");
        if generated_docs.exists() {
            fs::remove_dir_all(&generated_docs).map_err(|error| {
                format!("failed to clear '{}': {error}", generated_docs.display())
            })?;
        }
        let status = cargo
            .command()
            .args(["doc", "--workspace", "--no-deps", "--manifest-path"])
            .arg(&cargo_manifest)
            .env("CARGO_TARGET_DIR", &target)
            .current_dir(paths.source().root())
            .status()
            .map_err(|error| format!("failed to build {} docs: {error}", section.name()))?;
        if !status.success() {
            return Err(format!(
                "documentation build for '{}' failed with {status}",
                section.name()
            ));
        }
        let section_root = staged_root.join(section.name());
        copy_tree(&generated_docs, &section_root)?;
        copy_linked_markdown_docs(&cargo_manifest, &section_root)?;
        write_section_index(&section_root, section.name())?;
    }
    write_index(staged_root, manifest)?;
    let generated_docs = target.join("doc");
    if generated_docs.exists() {
        fs::remove_dir_all(&generated_docs).map_err(|error| {
            format!(
                "failed to clear temporary Rustdoc output '{}': {error}",
                generated_docs.display()
            )
        })?;
    }
    for section in manifest
        .cargo_projects()
        .iter()
        .filter(|project| project.documentation())
    {
        let legacy_target = paths
            .installation()
            .root()
            .join("output/docs")
            .join(section.name());
        if legacy_target.is_dir() {
            fs::remove_dir_all(&legacy_target).map_err(|error| {
                format!(
                    "failed to remove obsolete docs cache '{}': {error}",
                    legacy_target.display()
                )
            })?;
        }
    }
    install_documentation_site(staged_root, &docs_root, installation_root)?;
    Ok(docs_root)
}

fn documentation_target_root(installation_root: &Path) -> PathBuf {
    installation_root.join("output/docs/target")
}

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn create(parent: &Path, label: &str) -> Result<Self, String> {
        for attempt in 0..16 {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = parent.join(format!(".{label}-{}-{stamp}-{attempt}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!(
                        "could not create documentation staging directory '{}': {error}",
                        path.display()
                    ));
                }
            }
        }
        Err("could not reserve a documentation staging directory".to_owned())
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn install_documentation_site(
    staged: &Path,
    destination: &Path,
    installation_root: &Path,
) -> Result<(), String> {
    let backup = if destination.exists() {
        let backup = unused_path(installation_root, "docs-backup")?;
        fs::rename(destination, &backup).map_err(|error| {
            format!(
                "could not preserve existing documentation '{}': {error}",
                destination.display()
            )
        })?;
        Some(backup)
    } else {
        None
    };

    if let Err(error) = fs::rename(staged, destination) {
        if let Some(backup) = &backup {
            fs::rename(backup, destination).map_err(|restore_error| {
                format!(
                    "could not install generated documentation ({error}) or restore previous documentation '{}': {restore_error}",
                    backup.display()
                )
            })?;
        }
        return Err(format!(
            "could not install generated documentation '{}': {error}",
            destination.display()
        ));
    }

    if let Some(backup) = backup {
        fs::remove_dir_all(&backup).map_err(|error| {
            format!(
                "documentation was installed, but could not remove previous copy '{}': {error}",
                backup.display()
            )
        })?;
    }
    Ok(())
}

fn unused_path(parent: &Path, label: &str) -> Result<PathBuf, String> {
    for attempt in 0..16 {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = parent.join(format!(".{label}-{}-{stamp}-{attempt}", std::process::id()));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err("could not reserve a documentation backup path".to_owned())
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|error| error.to_string())?;
    for entry in fs::read_dir(source)
        .map_err(|error| format!("failed to read '{}': {error}", source.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        let destination = target.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            copy_tree(&entry.path(), &destination)?;
        } else {
            fs::copy(entry.path(), destination).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn copy_linked_markdown_docs(cargo_manifest: &Path, section_root: &Path) -> Result<(), String> {
    let Some(project_root) = cargo_manifest.parent() else {
        return Ok(());
    };
    let source_docs = project_root.join("docs");
    if !source_docs.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(section_root)
        .map_err(|error| format!("failed to read '{}': {error}", section_root.display()))?
    {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            continue;
        }
        let crate_root = entry.path();
        if crate_root.join("index.html").is_file() {
            copy_tree(&source_docs, &crate_root.join("docs"))?;
        }
    }
    Ok(())
}

fn write_index(root: &Path, manifest: &WorkspaceManifest) -> Result<(), String> {
    let links = manifest
        .cargo_projects()
        .iter()
        .filter(|project| project.documentation())
        .map(|section| {
            format!(
                "<li><a href=\"{0}/index.html\">{0}</a></li>",
                section.name()
            )
        })
        .collect::<String>();
    fs::write(
        root.join("index.html"),
        format!("<!doctype html><title>Vapor documentation</title><h1>Vapor documentation</h1><ul>{links}</ul>"),
    )
    .map_err(|error| error.to_string())
}

fn write_section_index(root: &Path, name: &str) -> Result<(), String> {
    let mut crates = fs::read_dir(root)
        .map_err(|error| {
            format!(
                "failed to read documentation root '{}': {error}",
                root.display()
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to read documentation entry: {error}"))?;
    crates.sort_by_key(|entry| entry.file_name());
    let links = crates
        .into_iter()
        .filter_map(|entry| {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().into_owned();
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .and_then(|_| path.join("index.html").is_file().then_some(file_name))
        })
        .map(|crate_name| {
            format!(
                "<li><a href=\"{0}/index.html\">{0}</a></li>",
                html_escape(&crate_name)
            )
        })
        .collect::<String>();
    let title = html_escape(name);
    fs::write(
        root.join("index.html"),
        format!(
            "<!doctype html><title>{title} documentation</title><h1>{title} documentation</h1><ul>{links}</ul>"
        ),
    )
    .map_err(|error| error.to_string())
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::{documentation_target_root, install_documentation_site};
    use std::{fs, path::Path, time::SystemTime};

    #[test]
    fn failed_documentation_swap_preserves_the_previous_site() {
        let stamp = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("vapor-docs-swap-{stamp}"));
        let previous = root.join("docs");
        let staged = root.join("staged");
        fs::create_dir_all(&previous).expect("create previous docs");
        fs::create_dir_all(&staged).expect("create staged docs");
        fs::write(previous.join("index.html"), "previous").expect("write previous docs");
        fs::write(staged.join("index.html"), "staged").expect("write staged docs");

        install_documentation_site(&staged, &previous, &root).expect("install staged docs");
        assert_eq!(
            fs::read_to_string(previous.join("index.html")).expect("read installed docs"),
            "staged"
        );

        let missing_staging = root.join("missing-staging");
        assert!(install_documentation_site(&missing_staging, &previous, &root).is_err());
        assert_eq!(
            fs::read_to_string(previous.join("index.html")).expect("read preserved docs"),
            "staged"
        );
        fs::remove_dir_all(root).expect("remove temporary docs tree");
    }

    #[test]
    fn documentation_projects_share_one_cargo_target_root() {
        assert_eq!(
            documentation_target_root(Path::new("/vapor")),
            Path::new("/vapor/output/docs/target")
        );
    }
}
