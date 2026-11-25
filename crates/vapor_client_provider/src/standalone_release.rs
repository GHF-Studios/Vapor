//! Staging of the Vapor-only standalone application payload.

use crate::workspace::WorkspaceManifest;
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Serialize)]
struct PackagedToolchainMetadata<'a> {
    schema: u32,
    channel: &'a str,
    version: &'a str,
    date: &'a str,
}

/// Stage already-built release outputs as a standalone app without changing
/// the active Vapor installation.
pub fn stage_standalone_release(
    source_root: &Path,
    output_root: &Path,
    user_data_root: &Path,
    manifest: &WorkspaceManifest,
    target_triples: &[String],
) -> Result<(PathBuf, usize), String> {
    if target_triples.is_empty() {
        return Err("standalone release staging needs at least one runtime target".to_owned());
    }

    let final_root = output_root.join("standalone/release");
    let parent = final_root
        .parent()
        .ok_or_else(|| "standalone staging root has no parent".to_owned())?;
    fs::create_dir_all(output_root)
        .map_err(|error| format!("failed to create '{}': {error}", output_root.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create '{}': {error}", parent.display()))?;
    let canonical_output = fs::canonicalize(&output_root)
        .map_err(|error| format!("failed to resolve '{}': {error}", output_root.display()))?;
    let canonical_parent = fs::canonicalize(parent)
        .map_err(|error| format!("failed to resolve '{}': {error}", parent.display()))?;
    if !canonical_parent.starts_with(&canonical_output) {
        return Err(format!(
            "standalone staging directory escaped the configured output root: {}",
            canonical_parent.display()
        ));
    }
    let staging_root = parent.join(format!(".release-staging-{}", unique_suffix()?));
    fs::create_dir(&staging_root)
        .map_err(|error| format!("failed to create '{}': {error}", staging_root.display()))?;

    let promoted = match stage_into(
        source_root,
        output_root,
        user_data_root,
        manifest,
        target_triples,
        &staging_root,
    ) {
        Ok(promoted) => promoted,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging_root);
            return Err(error);
        }
    };

    let backup_root = match unique_suffix() {
        Ok(suffix) => parent.join(format!(".release-previous-{suffix}")),
        Err(error) => {
            let _ = fs::remove_dir_all(&staging_root);
            return Err(error);
        }
    };
    let had_previous = match fs::symlink_metadata(&final_root) {
        Ok(metadata) if metadata.file_type().is_dir() => true,
        Ok(_) => {
            let _ = fs::remove_dir_all(&staging_root);
            return Err(format!(
                "refusing to replace a non-directory standalone stage: {}",
                final_root.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging_root);
            return Err(format!(
                "failed to inspect previous standalone stage '{}': {error}",
                final_root.display()
            ));
        }
    };
    if had_previous {
        fs::rename(&final_root, &backup_root).map_err(|error| {
            format!(
                "failed to preserve the previous standalone stage '{}': {error}",
                final_root.display()
            )
        })?;
    }
    if let Err(error) = fs::rename(&staging_root, &final_root) {
        if had_previous {
            let _ = fs::rename(&backup_root, &final_root);
        }
        let _ = fs::remove_dir_all(&staging_root);
        return Err(format!(
            "failed to activate standalone stage '{}': {error}",
            final_root.display()
        ));
    }
    if had_previous {
        fs::remove_dir_all(&backup_root).map_err(|error| {
            format!(
                "standalone stage is ready at '{}', but previous stage '{}' could not be removed: {error}",
                final_root.display(),
                backup_root.display()
            )
        })?;
    }

    Ok((final_root, promoted))
}

fn stage_into(
    source_root: &Path,
    output_root: &Path,
    user_data_root: &Path,
    manifest: &WorkspaceManifest,
    targets: &[String],
    staging_root: &Path,
) -> Result<usize, String> {
    let vapor_workspace = vapor_core::VaporWorkspace::discover_from(&source_root.join("Vapor"))
        .map_err(|error| format!("failed to load Vapor's canonical toolchain pin: {error}"))?;
    let pin = &vapor_workspace.manifest.toolchain;
    let metadata = PackagedToolchainMetadata {
        schema: 1,
        channel: &pin.channel,
        version: &pin.version,
        date: &pin.date,
    };
    let metadata_path = staging_root.join("metadata/toolchain.toml");
    fs::create_dir_all(metadata_path.parent().expect("metadata path has parent"))
        .map_err(|error| format!("failed to create packaged metadata directory: {error}"))?;
    let serialized = toml::to_string_pretty(&metadata)
        .map_err(|error| format!("failed to serialize packaged toolchain pin: {error}"))?;
    fs::write(&metadata_path, serialized)
        .map_err(|error| format!("failed to write '{}': {error}", metadata_path.display()))?;

    copy_required(
        &source_root.join("Standalone-App.vapor.toml"),
        &staging_root.join("App.vapor.toml"),
    )?;
    copy_tree(
        &source_root.join("resources/vapor/tools"),
        &staging_root.join("resources/vapor/tools"),
    )?;

    for target in targets {
        let (platform, launcher) = if target.contains("windows") {
            ("windows", "vapor-launch.cmd")
        } else if target.contains("linux") {
            ("linux", "vapor-launch.sh")
        } else {
            return Err(format!(
                "standalone release staging does not support runtime target '{target}'"
            ));
        };
        let source = source_root
            .join("resources/vapor/shell-scripts")
            .join(platform)
            .join(launcher);
        let destination = staging_root.join("bin").join(launcher);
        copy_required(&source, &destination)?;
        if platform == "linux" {
            make_executable(&destination)?;
        }
    }

    crate::release::promotion::stage_release_for_targets_from_roots(
        output_root,
        user_data_root,
        manifest,
        targets,
        staging_root,
    )
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .map_err(|error| format!("failed to inspect '{}': {error}", path.display()))?
        .permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    fs::set_permissions(path, permissions)
        .map_err(|error| format!("failed to mark '{}' executable: {error}", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn copy_required(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| {
        format!(
            "required standalone input '{}' is unavailable: {error}",
            source.display()
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "required standalone input is not a regular file: {}",
            source.display()
        ));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create '{}': {error}", parent.display()))?;
    }
    fs::copy(source, destination).map_err(|error| {
        format!(
            "failed to copy '{}' to '{}': {error}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| {
        format!(
            "required standalone directory '{}' is unavailable: {error}",
            source.display()
        )
    })?;
    if !metadata.file_type().is_dir() {
        return Err(format!(
            "required standalone directory is not a real directory: {}",
            source.display()
        ));
    }
    fs::create_dir_all(destination)
        .map_err(|error| format!("failed to create '{}': {error}", destination.display()))?;
    for entry in fs::read_dir(source)
        .map_err(|error| format!("failed to read '{}': {error}", source.display()))?
    {
        let entry = entry
            .map_err(|error| format!("failed to read entry in '{}': {error}", source.display()))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|error| format!("failed to inspect '{}': {error}", source_path.display()))?;
        if kind.is_symlink() {
            return Err(format!(
                "standalone release source contains a symbolic link: {}",
                source_path.display()
            ));
        } else if kind.is_dir() {
            copy_tree(&source_path, &destination_path)?;
        } else if kind.is_file() {
            copy_required(&source_path, &destination_path)?;
        } else {
            return Err(format!(
                "unsupported standalone release source entry: {}",
                source_path.display()
            ));
        }
    }
    Ok(())
}

fn unique_suffix() -> Result<String, String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before Unix epoch: {error}"))?
        .as_nanos();
    Ok(format!("{}-{nanos}", std::process::id()))
}
