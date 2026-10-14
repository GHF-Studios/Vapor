use std::env;
use std::path::{Path, PathBuf};

use toml::Value;

use crate::GlobalOptions;

use super::error::WorkspaceCommandError;

pub(super) const WORKSPACE_MANIFEST_FILE: &str = "Workspace.vapor.toml";

#[derive(Debug, Clone)]
pub(super) struct WorkspaceIdentity {
    pub(super) invocation_directory: PathBuf,
    pub(super) workspace_root: PathBuf,
    pub(super) name: String,
    pub(super) id: String,
}

pub(super) fn discover_workspace_identity(
    globals: &GlobalOptions,
) -> Result<WorkspaceIdentity, WorkspaceCommandError> {
    let invocation_directory = env::current_dir()?;
    let mut candidate = workspace_search_start(&invocation_directory, globals)?;

    loop {
        let manifest_path = candidate.join(WORKSPACE_MANIFEST_FILE);
        if manifest_path.is_file() {
            let text = std::fs::read_to_string(&manifest_path)?;
            let manifest = text.parse::<Value>()?;
            let workspace = manifest.get("workspace").ok_or_else(|| {
                WorkspaceCommandError::InvalidWorkspaceManifest(
                    "Workspace.vapor.toml must declare [workspace]".to_owned(),
                )
            })?;
            let name = workspace
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    WorkspaceCommandError::InvalidWorkspaceManifest(
                        "[workspace].name is required".to_owned(),
                    )
                })?
                .to_owned();
            let organization = workspace
                .get("organization")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    WorkspaceCommandError::InvalidWorkspaceManifest(
                        "[workspace].organization is required".to_owned(),
                    )
                })?;
            let id = format!("{organization}/{name}");

            return Ok(WorkspaceIdentity {
                invocation_directory,
                workspace_root: candidate,
                id,
                name,
            });
        }

        if !candidate.pop() {
            return Err(WorkspaceCommandError::MissingWorkspaceManifest(
                invocation_directory,
            ));
        }
    }
}

fn workspace_search_start(
    invocation_directory: &Path,
    globals: &GlobalOptions,
) -> Result<PathBuf, WorkspaceCommandError> {
    let Some(workspace) = &globals.workspace else {
        return Ok(invocation_directory.to_path_buf());
    };

    if workspace.is_absolute() {
        Ok(workspace.clone())
    } else {
        Ok(invocation_directory.join(workspace))
    }
}
