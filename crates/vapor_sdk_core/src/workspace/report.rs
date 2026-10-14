use std::path::PathBuf;
use std::process::ExitStatus;

/// Result of running one Vapor-managed Cargo command.
#[derive(Debug, Clone)]
pub struct WorkspaceCargoReport {
    pub invocation_directory: PathBuf,
    pub workspace_root: PathBuf,
    pub workspace_name: String,
    pub workspace_id: String,
    pub working_directory: PathBuf,
    pub cargo_path: PathBuf,
    pub rustc_path: PathBuf,
    pub cargo_home: PathBuf,
    pub target_dir: PathBuf,
    pub cargo_args: Vec<String>,
    pub status: ExitStatus,
}

/// Result of promoting a built SDK executable into the deploy root.
#[derive(Debug, Clone)]
pub struct WorkspaceDeployReport {
    pub build: WorkspaceCargoReport,
    pub source_executable: PathBuf,
    pub deployed_executable: PathBuf,
    pub alias_executable: PathBuf,
    pub activation_script: PathBuf,
}

/// Current SDK-managed workspace structure state.
#[derive(Debug, Clone)]
pub struct WorkspaceStatusReport {
    pub invocation_directory: PathBuf,
    pub workspace_root: PathBuf,
    pub workspace_name: String,
    pub workspace_id: String,
    pub cargo_manifest_exists: bool,
    pub crates_dir_exists: bool,
}

/// Result of syncing managed workspace structure.
#[derive(Debug, Clone)]
pub struct WorkspaceSyncReport {
    pub status: WorkspaceStatusReport,
    pub changed_paths: Vec<PathBuf>,
}
