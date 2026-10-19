//! Installation of app-owned scripts, developer tools, and launch shims.

use crate::{
    discovery::{EnvironmentPaths, ensure_contained},
    target::host_runtime_target,
};
use std::{collections::BTreeSet, fs, path::Path};

/// Counts of source-owned files copied into the app installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootAssetReport {
    /// Vapor-owned interactive shell scripts installed.
    pub scripts: usize,
    /// App-root tool payload files installed.
    pub tools: usize,
    /// Platform-specific launch scripts installed.
    pub launch_scripts: usize,
}

/// Synchronize source-owned launch assets into the app installation.
pub fn sync_release_assets(
    paths: &EnvironmentPaths,
    targets: &[String],
) -> Result<RootAssetReport, String> {
    Ok(RootAssetReport {
        scripts: sync_source_tree(
            paths,
            "resources/vapor/vapor-scripts",
            "resources/vapor/vapor-scripts",
            true,
        )?,
        tools: sync_source_tree(
            paths,
            "resources/vapor/tools",
            "resources/vapor/tools",
            false,
        )?,
        launch_scripts: sync_launch_scripts(paths, targets)?,
    })
}

fn sync_source_tree(
    paths: &EnvironmentPaths,
    source_relative: &str,
    target_relative: &str,
    clear_when_missing: bool,
) -> Result<usize, String> {
    let source = paths.source().root().join(source_relative);
    let target = paths.installation().root().join(target_relative);
    ensure_contained(paths.installation().root(), &target)?;
    if source.is_dir() {
        let canonical =
            fs::canonicalize(&source).map_err(|error| path_error("resolve", &source, error))?;
        ensure_contained(paths.source().root(), &canonical)?;
        if target.exists() {
            fs::remove_dir_all(&target).map_err(|error| path_error("reset", &target, error))?;
        }
        copy_tree(&canonical, &target)
    } else if clear_when_missing && target.exists() {
        fs::remove_dir_all(&target).map_err(|error| path_error("reset", &target, error))?;
        Ok(0)
    } else {
        Ok(0)
    }
}

fn sync_launch_scripts(paths: &EnvironmentPaths, targets: &[String]) -> Result<usize, String> {
    let source = paths.source().root().join("resources/vapor/shell-scripts");
    let target = paths.installation().root().join("bin");
    ensure_contained(paths.installation().root(), &target)?;
    if !source.exists() {
        return Ok(0);
    }
    let canonical =
        fs::canonicalize(&source).map_err(|error| path_error("resolve", &source, error))?;
    ensure_contained(paths.source().root(), &canonical)?;
    let mut files = 0;
    for platform in target_platforms(targets) {
        let (relative_source, target_name) = match platform.as_str() {
            "linux" => ("linux/vapor-launch.sh", "vapor-launch.sh"),
            "windows" => ("windows/vapor-launch.cmd", "vapor-launch.cmd"),
            other => {
                return Err(format!(
                    "no launch script mapping exists for platform '{other}'"
                ));
            }
        };
        let script_source = canonical.join(relative_source);
        if script_source.is_file() {
            let script_target = target.join(target_name);
            fs::copy(&script_source, &script_target)
                .map_err(|error| path_error("copy", &script_source, error))?;
            make_executable(&script_target)?;
            files += 1;
        }
    }
    Ok(files)
}

fn target_platforms(targets: &[String]) -> BTreeSet<String> {
    let selected = if targets.is_empty() {
        vec![host_runtime_target()]
    } else {
        targets.to_vec()
    };
    selected
        .into_iter()
        .filter_map(|target| {
            if target.contains("linux") {
                Some("linux".to_owned())
            } else if target.contains("windows") {
                Some("windows".to_owned())
            } else {
                None
            }
        })
        .collect()
}

fn copy_tree(source: &Path, target: &Path) -> Result<usize, String> {
    fs::create_dir_all(target).map_err(|error| path_error("create", target, error))?;
    let mut files = 0;
    for entry in fs::read_dir(source).map_err(|error| path_error("read", source, error))? {
        let entry = entry.map_err(|error| error.to_string())?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_dir() {
            files += copy_tree(&source_path, &target_path)?;
        } else if file_type.is_file() {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent).map_err(|error| path_error("create", parent, error))?;
            }
            fs::copy(&source_path, &target_path)
                .map_err(|error| path_error("copy", &source_path, error))?;
            files += 1;
        }
    }
    Ok(files)
}

fn make_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .map_err(|error| path_error("inspect", path, error))?
            .permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        fs::set_permissions(path, permissions)
            .map_err(|error| path_error("make executable", path, error))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn path_error(action: &str, path: &Path, error: std::io::Error) -> String {
    format!("failed to {action} '{}': {error}", path.display())
}
