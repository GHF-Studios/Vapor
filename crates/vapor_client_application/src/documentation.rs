//! Service-owned documentation generation for an opened source workspace.

use std::path::Path;
use vapor_client_provider::{
    discovery::{EnvironmentPaths, InstallationPaths},
    workspace::WorkspaceManifest,
};

/// Build and install Rustdoc for the documentation-enabled projects in a workspace.
pub fn build_documentation_with_context(
    application_executable: &Path,
    workspace_root: &Path,
    context: &vapor_core::OperationContext,
) -> Result<String, String> {
    if context.is_cancelled() {
        return Err("documentation build was cancelled before it started".to_owned());
    }
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    let manifest = WorkspaceManifest::load(&paths)?;
    context.set_phase(vapor_core::OperationPhase::Building);
    let output = vapor_client_provider::documentation::build(&paths, &manifest)?;
    if context.is_cancelled() {
        return Err("documentation build was cancelled after completion".to_owned());
    }
    Ok(format!("Documentation built at {}.", output.display()))
}
