use crate::GlobalOptions;
use crate::toolchain::{ToolchainInstallState, ToolchainStatus, toolchain_status};
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use vapor_core::RustToolchainCommandEnvironment;

use super::error::WorkspaceCommandError;
use super::identity::{WorkspaceIdentity, discover_workspace_identity};
use super::report::WorkspaceCargoReport;

const DEV_OUTPUT_DIR: &str = "dev";

#[derive(Debug, Clone)]
pub(super) struct VaporCargo {
    pub(super) toolchain: ToolchainStatus,
    pub(super) target_dir: PathBuf,
    pub(super) identity: WorkspaceIdentity,
    cargo_home: PathBuf,
}

impl VaporCargo {
    pub(super) fn new(globals: &GlobalOptions) -> Result<Self, WorkspaceCommandError> {
        let toolchain = checked_toolchain_status()?;
        let identity = discover_workspace_identity(globals)?;
        let cargo_home = toolchain.cargo_home.clone();
        let target_dir = toolchain
            .output_root
            .join(DEV_OUTPUT_DIR)
            .join(workspace_output_name(&identity));

        Ok(Self {
            toolchain,
            identity,
            cargo_home,
            target_dir,
        })
    }

    pub(super) fn run(&self, args: &[&str]) -> Result<WorkspaceCargoReport, WorkspaceCommandError> {
        let mut command =
            vapor_core::managed_process::background_command(&self.toolchain.cargo_path);
        RustToolchainCommandEnvironment::new(
            &self.toolchain.vapor_home,
            &self.cargo_home,
            &self.toolchain.rustup_home,
            self.toolchain.toolchain.identifier(),
            &self.toolchain.rustc_path,
            self.toolchain
                .cargo_path
                .with_file_name(format!("rustdoc{}", env::consts::EXE_SUFFIX)),
        )
        .apply_to(&mut command);
        command
            .args(args)
            .current_dir(&self.identity.workspace_root)
            .env("CARGO_TARGET_DIR", &self.target_dir)
            .env("PATH", vapor_path_prefix(&self.toolchain)?);

        let status = command.status()?;

        Ok(WorkspaceCargoReport {
            invocation_directory: self.identity.invocation_directory.clone(),
            workspace_root: self.identity.workspace_root.clone(),
            workspace_name: self.identity.name.clone(),
            workspace_id: self.identity.id.clone(),
            working_directory: self.identity.workspace_root.clone(),
            cargo_path: self.toolchain.cargo_path.clone(),
            rustc_path: self.toolchain.rustc_path.clone(),
            cargo_home: self.cargo_home.clone(),
            target_dir: self.target_dir.clone(),
            cargo_args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            status,
        })
    }
}

fn workspace_output_name(identity: &WorkspaceIdentity) -> String {
    let raw = identity.id.as_str();

    let name = raw
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned();

    if name.is_empty() {
        "workspace".to_owned()
    } else {
        name
    }
}

fn checked_toolchain_status() -> Result<ToolchainStatus, WorkspaceCommandError> {
    let toolchain = toolchain_status()?;

    if !toolchain.host_supported {
        return Err(WorkspaceCommandError::UnsupportedHost(
            toolchain.host_triple.to_owned(),
        ));
    }

    if !matches!(
        toolchain.install_state,
        ToolchainInstallState::PresentUnverified
    ) {
        return Err(WorkspaceCommandError::ToolchainNotInstalled(
            toolchain.install_state,
        ));
    }

    Ok(toolchain)
}

fn vapor_path_prefix(toolchain: &ToolchainStatus) -> Result<OsString, WorkspaceCommandError> {
    let cargo_bin_dir = toolchain
        .cargo_path
        .parent()
        .ok_or_else(|| WorkspaceCommandError::CargoPathHasNoParent(toolchain.cargo_path.clone()))?;
    let mut paths = vec![cargo_bin_dir.to_path_buf()];

    if let Some(rustup_bin_dir) = toolchain
        .rustup_path
        .as_ref()
        .and_then(|path| path.parent())
    {
        paths.push(rustup_bin_dir.to_path_buf());
    }

    if let Some(existing) = env::var_os("PATH") {
        paths.extend(env::split_paths(&existing));
    }
    for host_tools in host_tool_paths() {
        if !paths.contains(&host_tools) {
            paths.push(host_tools);
        }
    }

    Ok(env::join_paths(paths)?)
}

#[cfg(unix)]
fn host_tool_paths() -> Vec<PathBuf> {
    [
        "/usr/local/sbin",
        "/usr/local/bin",
        "/usr/sbin",
        "/usr/bin",
        "/sbin",
        "/bin",
    ]
    .into_iter()
    .map(PathBuf::from)
    .filter(|path| path.is_dir())
    .collect()
}

#[cfg(not(unix))]
fn host_tool_paths() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(all(test, unix))]
mod tests {
    use super::host_tool_paths;
    use std::path::Path;

    #[test]
    fn managed_cargo_path_includes_host_tool_directories() {
        let paths = host_tool_paths();

        for directory in ["/usr/bin", "/bin"] {
            let directory = Path::new(directory);
            if directory.is_dir() {
                assert!(paths.iter().any(|path| path == directory));
            }
        }
    }
}
