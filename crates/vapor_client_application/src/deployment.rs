//! Local deployment of the first-party Vapor application.

use std::path::Path;
use vapor_client_provider::discovery::{EnvironmentPaths, InstallationPaths};

/// Promote built Vapor binaries and support assets into the installed app root.
///
/// This operation does not build source. It promotes the selected, already-built
/// target outputs, optionally refreshes Rustdoc, and synchronizes app-root assets.
pub fn deploy_application_with_context(
    application_executable: &Path,
    workspace_root: &Path,
    target_triples: &[String],
    skip_documentation: bool,
    context: &vapor_core::OperationContext,
) -> Result<String, String> {
    if context.is_cancelled() {
        return Err("deployment was cancelled before promotion began".to_owned());
    }
    if !workspace_root.join("Vapor-Standalone.toml").is_file() {
        return Err(format!(
            "root deploy is only for Vapor standalone source roots; use `content deploy` for authored game or engine products ({})",
            workspace_root.display()
        ));
    }
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    let manifest = vapor_client_provider::workspace::WorkspaceManifest::load(&paths)?;

    context.set_phase(vapor_core::OperationPhase::Staging);
    let promoted = vapor_client_provider::release::promotion::promote_for_targets(
        &paths,
        &manifest,
        target_triples,
    )?;
    if context.is_cancelled() {
        return Err("deployment was cancelled after binary promotion".to_owned());
    }

    context.set_phase(vapor_core::OperationPhase::Building);
    let documentation = if skip_documentation {
        None
    } else {
        Some(vapor_client_provider::documentation::build(
            &paths, &manifest,
        )?)
    };
    if context.is_cancelled() {
        return Err("deployment was cancelled after documentation".to_owned());
    }

    context.set_phase(vapor_core::OperationPhase::Staging);
    let assets = vapor_client_provider::app_root::sync_release_assets(&paths, target_triples)?;
    let documentation_summary = documentation.map_or_else(
        || "documentation skipped".to_owned(),
        |path| format!("documentation refreshed at {}", path.display()),
    );
    Ok(format!(
        "Promoted {promoted} runtime artifact(s); {documentation_summary}; synchronized {} script(s), {} tool(s), {} SteamPipe template(s), and {} launch script(s).",
        assets.scripts, assets.tools, assets.steam_templates, assets.launch_scripts
    ))
}

/// Build and stage the selected standalone runtime and/or optional Steam depots.
pub fn package_application_with_context(
    application_executable: &Path,
    workspace_root: &Path,
    target_triples: &[String],
    context: &vapor_core::OperationContext,
) -> Result<String, String> {
    if target_triples.is_empty() {
        return Err("application packaging needs at least one runtime target".to_owned());
    }
    let installation = InstallationPaths::from_application_executable(application_executable)?;
    let paths = EnvironmentPaths::from_installation_and_source_path(installation, workspace_root)?;
    let workspace_manifest = vapor_client_provider::workspace::WorkspaceManifest::load(&paths)?;

    let has_installable_cargo = workspace_manifest
        .cargo_projects()
        .iter()
        .any(|project| !project.binaries().is_empty());
    let mut promoted = 0;
    if has_installable_cargo {
        context.set_phase(vapor_core::OperationPhase::Building);
        crate::run_workspace_tool_with_context(
            application_executable,
            workspace_root,
            vapor_client_domain::WorkspaceToolOperation::BuildRelease,
            &vapor_client_domain::WorkspaceProjectSelection::Installable,
            target_triples,
            context,
        )?;
        if context.is_cancelled() {
            return Err("application packaging was cancelled after its release build".to_owned());
        }

        context.set_phase(vapor_core::OperationPhase::Staging);
        promoted = vapor_client_provider::release::promotion::promote_release_for_targets(
            &paths,
            &workspace_manifest,
            target_triples,
        )?;
        context.set_phase(vapor_core::OperationPhase::Building);
        vapor_client_provider::documentation::build(&paths, &workspace_manifest)?;
        if context.is_cancelled() {
            return Err("application packaging was cancelled after documentation".to_owned());
        }
    }

    let has_standalone_payload = paths
        .source()
        .root()
        .join("Vapor-Standalone.toml")
        .is_file();
    context.set_phase(vapor_core::OperationPhase::Staging);
    let assets = if has_standalone_payload {
        Some(vapor_client_provider::app_root::sync_release_assets(
            &paths,
            target_triples,
        )?)
    } else {
        None
    };
    let distribution =
        vapor_client_provider::distribution::DistributionManifest::load_optional(&paths)?;
    if !has_standalone_payload && distribution.is_none() {
        return Err(format!(
            "source root '{}' declares neither a standalone Vapor payload nor Steam depot policy",
            paths.source().root().display()
        ));
    }
    let options = vapor_client_provider::distribution::StageOptions::runtime()
        .with_runtime_targets(target_triples.to_vec());
    let staged = if let Some(distribution) = &distribution {
        let staged = vapor_client_provider::distribution::stage_with_options(
            &paths,
            distribution,
            options.clone(),
        )?;
        vapor_client_provider::steam::smoke(&staged, distribution, &options)?;
        Some(staged)
    } else {
        None
    };
    let standalone = if has_standalone_payload {
        Some(vapor_client_provider::distribution::stage_standalone_bundles(&paths, target_triples)?)
    } else {
        None
    };

    let mut summary = Vec::new();
    if promoted > 0 {
        summary.push(format!("promoted {promoted} release artifact(s)"));
    }
    if let Some(staged) = staged {
        summary.push(format!(
            "staged {} Steam depot file(s) at {}",
            staged.files(),
            staged.root().display()
        ));
    }
    if let Some((standalone_root, standalone_files)) = standalone {
        summary.push(format!(
            "staged {standalone_files} standalone bundle file(s) under {}",
            standalone_root.display()
        ));
    }
    if let Some(assets) = assets {
        summary.push(format!(
            "synchronized {} script(s), {} tool(s), {} SteamPipe template(s), and {} launch script(s)",
            assets.scripts, assets.tools, assets.steam_templates, assets.launch_scripts
        ));
    }
    summary.push(format!("targets: {}", target_triples.join(", ")));
    Ok(summary.join("; "))
}
