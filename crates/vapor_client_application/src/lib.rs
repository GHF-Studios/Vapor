//! Vapor Client application operations and source-workspace adapters.
//!
//! Content workflows consume Vapor-owned domain and provider operations. The
//! Vapor Client service owns workflow state and orchestration around them.

mod catalog;
mod composition;
mod content;
mod deployment;
mod documentation;
mod operation;
mod publishing;
mod workspace;
mod workspace_files;

pub use catalog::{
    CatalogConflict, CatalogContent, CatalogDependency, CatalogLocation, CatalogLocationKind,
    discover_local_content_catalog,
};
pub use composition::{
    CURRENT_SCHEMA_VERSION, Composition, CompositionDocument, CompositionRecovery,
    ContentReference, InstallationState, InstalledArtifactRecord, LaunchSettings,
    ResolvedCompositionGraph, ResolvedDependencyIdentity, ResolvedNodeIdentity,
    WorkspaceAssociation, add_composition, decode_compositions, duplicate_composition,
    fingerprint_resolved_graph, remove_composition, rename_composition, resolved_graph_matches,
};
pub use content::{
    CompositionPreview, CompositionPreviewDependency, CompositionPreviewNode,
    CompositionPreviewSource, CompositionRunResult, InstallationCheck, InstalledArtifact,
    InstalledComposition, WorkshopAcquisition, WorkshopAcquisitionBatch, collect_library_item,
    default_packagepack_selector, download_workshop_item, download_workshop_items,
    download_workshop_items_with_constraints, install_composition, package_authored_content,
    preview_composition, repair_composition, run_installed_composition, run_library_operation,
    run_source_composition, update_composition, verify_installation,
};
pub use deployment::{deploy_application_with_context, package_application_with_context};
pub use documentation::build_documentation_with_context;
pub use documentation::publish_documentation_with_context;
pub use operation::{
    CoreOperationEvent, CoreOperationRequest, OperationCancellation, OperationContext,
    OperationEvent, OperationEventSink, OperationId, OperationKind, OperationOutput,
    OperationOutputStream, OperationPhase, OperationRequest, OperationService, OperationStatus,
    Progress,
};
pub use publishing::{
    ApplicationReleaseResult, ApplicationReleaseReview, BuildMode, BuildReport,
    MAX_WORKSHOP_REVIEW_BATCH, WorkshopBatchReview, WorkshopPackagePreview, WorkshopPublication,
    build_and_package_authored_content_with_context, build_application_release,
    build_application_release_with_events, build_authored_workspace,
    clean_workspace_build_cache_with_context, publish_reviewed_application_release,
    publish_reviewed_workshop_batch, publish_reviewed_workshop_content,
    resolve_workspace_runtime_targets, run_workspace_tool, run_workspace_tool_with_context,
    run_workspace_tool_with_events, run_workspace_tool_with_progress, stage_application_release,
    stage_workshop_batch_preview, stage_workshop_preview, validate_application_release_request,
};
pub use vapor_client_domain::can_auto_install_library_item;

fn profile_trace_snapshot(
    record: vapor_core::ProfileTraceRecord,
) -> vapor_client_protocol::ProfileTraceSnapshot {
    let available = record.exists();
    vapor_client_protocol::ProfileTraceSnapshot {
        id: record.id,
        path: record.path,
        available,
        added_unix_ms: record.added_unix_ms,
        workspace: record.workspace,
        session_id: record.session_id,
        configuration: record.configuration,
    }
}

/// Read the managed Tracy capture library for a frontend snapshot.
pub fn list_managed_profile_traces()
-> Result<Vec<vapor_client_protocol::ProfileTraceSnapshot>, String> {
    vapor_core::list_profile_traces()
        .map(|records| records.into_iter().map(profile_trace_snapshot).collect())
        .map_err(|error| error.to_string())
}

/// Add a Tracy capture to Vapor's managed profiler library.
pub fn import_managed_profile_trace(
    path: &std::path::Path,
) -> Result<vapor_client_protocol::ProfileTraceSnapshot, String> {
    vapor_core::import_profile_trace(path)
        .map(profile_trace_snapshot)
        .map_err(|error| error.to_string())
}

/// Analyze a saved Tracy capture using the managed exporter and shared options.
pub fn analyze_managed_profile_trace(
    record: &vapor_client_protocol::ProfileTraceSnapshot,
    options: &vapor_client_domain::ProfileAnalysisOptions,
) -> Result<vapor_client_protocol::ProfileReportSnapshot, String> {
    let record = vapor_core::ProfileTraceRecord {
        id: record.id.clone(),
        path: record.path.clone(),
        added_unix_ms: record.added_unix_ms,
        workspace: record.workspace.clone(),
        session_id: record.session_id.clone(),
        configuration: record.configuration.clone(),
    };
    let options = vapor_core::ProfileReportOptions {
        top: options.top,
        sort: match options.sort {
            vapor_client_domain::ProfileTraceSort::Total => vapor_core::ProfileSort::Total,
            vapor_client_domain::ProfileTraceSort::Count => vapor_core::ProfileSort::Count,
            vapor_client_domain::ProfileTraceSort::Mean => vapor_core::ProfileSort::Mean,
            vapor_client_domain::ProfileTraceSort::Median => vapor_core::ProfileSort::Median,
            vapor_client_domain::ProfileTraceSort::P90 => vapor_core::ProfileSort::P90,
            vapor_client_domain::ProfileTraceSort::P95 => vapor_core::ProfileSort::P95,
            vapor_client_domain::ProfileTraceSort::P99 => vapor_core::ProfileSort::P99,
            vapor_client_domain::ProfileTraceSort::Max => vapor_core::ProfileSort::Max,
        },
        thread_ids: Vec::new(),
        after_ms: None,
        before_ms: None,
        name_filter: options.name_filter.clone(),
        self_time: options.self_time,
    };
    let report =
        vapor_core::analyze_profile_trace(&record, &options).map_err(|error| error.to_string())?;
    Ok(vapor_client_protocol::ProfileReportSnapshot {
        trace: report.trace,
        self_time: report.self_time,
        occurrence_count: report.occurrence_count,
        zone_count: report.zone_count,
        zones: report
            .zones
            .into_iter()
            .map(|zone| vapor_client_protocol::ProfileZoneSnapshot {
                name: zone.name,
                source: zone.source,
                line: zone.line,
                threads: zone.threads,
                count: zone.count,
                total_ns: zone.total_ns,
                mean_ns: zone.mean_ns,
                median_ns: zone.median_ns,
                p90_ns: zone.p90_ns,
                p95_ns: zone.p95_ns,
                p99_ns: zone.p99_ns,
                max_ns: zone.max_ns,
            })
            .collect(),
    })
}

/// Open Vapor's managed Tracy profiler for the current installation.
pub fn open_managed_profiler() -> Result<(), String> {
    vapor_core::DeveloperEnvironment::discover()
        .and_then(|environment| environment.launch_tracy())
        .map_err(|error| error.to_string())
}

/// Read the role and managed-tool readiness for Vapor's active installation.
pub fn installation_role_snapshot() -> vapor_client_protocol::InstallationRoleSnapshot {
    let result = (|| {
        let installation =
            vapor_core::VaporInstallation::discover().map_err(|error| error.to_string())?;
        let role = vapor_core::installed_role(&installation).map_err(|error| error.to_string())?;
        let tools = vapor_core::DeveloperEnvironment::discover_for_installation(&installation)
            .map(|environment| environment.status())
            .map_err(|error| error.to_string());
        Ok(installation_role_snapshot_for(role.as_str(), tools))
    })();
    result.unwrap_or_else(|error| vapor_client_protocol::InstallationRoleSnapshot {
        error: Some(error),
        ..Default::default()
    })
}

fn installation_role_snapshot_for(
    role: &str,
    tools: Result<vapor_core::DeveloperEnvironmentStatus, String>,
) -> vapor_client_protocol::InstallationRoleSnapshot {
    match tools {
        Ok(tools) => vapor_client_protocol::InstallationRoleSnapshot {
            role: role.to_owned(),
            rust_ready: tools.rust_installed,
            rust_analyzer_ready: tools.rust_analyzer_installed,
            windows_gnu_target_ready: tools.windows_gnu_target_installed,
            windows_gnu_linker_ready: tools.windows_gnu_linker_installed,
            tracy_profiler_ready: tools.tracy_profiler_installed,
            tracy_analysis_ready: tools.tracy_analysis_installed,
            error: None,
        },
        Err(error) => vapor_client_protocol::InstallationRoleSnapshot {
            role: role.to_owned(),
            error: Some(error),
            ..Default::default()
        },
    }
}

/// Promote the active Vapor installation to an ordinary local role.
pub fn promote_installation_role(target: &str) -> Result<String, String> {
    promote_installation_role_inner(target, None)
}

/// Promote the active installation and report setup work to the shared
/// operation stream.
pub fn promote_installation_role_with_context(
    target: &str,
    context: &vapor_core::OperationContext,
) -> Result<String, String> {
    promote_installation_role_inner(target, Some(context))
}

fn promote_installation_role_inner(
    target: &str,
    context: Option<&vapor_core::OperationContext>,
) -> Result<String, String> {
    use std::str::FromStr as _;
    let installation =
        vapor_core::VaporInstallation::discover().map_err(|error| error.to_string())?;
    let target = vapor_core::VaporRole::from_str(target).map_err(|error| error.to_string())?;
    let report = match context {
        Some(context) => vapor_core::promote_role_with_context(&installation, target, context),
        None => vapor_core::promote_role(&installation, target),
    }
    .map_err(|error| error.to_string())?;
    Ok(format!(
        "Installation promoted from {} to {}.{}",
        report.previous_role,
        report.installed_role,
        if report.developer_environment_changed {
            " Developer tools were installed or repaired."
        } else {
            " Required tools were already ready."
        }
    ))
}

/// Demote the active Vapor installation without deleting its tools or caches.
pub fn demote_installation_role(target: &str) -> Result<String, String> {
    use std::str::FromStr as _;
    let installation =
        vapor_core::VaporInstallation::discover().map_err(|error| error.to_string())?;
    let target = vapor_core::VaporRole::from_str(target).map_err(|error| error.to_string())?;
    let report =
        vapor_core::demote_role(&installation, target).map_err(|error| error.to_string())?;
    Ok(format!(
        "Installation demoted from {} to {}. Tools and authored data were preserved.",
        report.previous_role, report.installed_role
    ))
}
pub use vapor_steam::{
    MAX_SUBSCRIPTION_BATCH_SIZE as MAX_WORKSHOP_SUBSCRIPTION_BATCH, search_workshop,
    subscribe_item as subscribe_workshop_item, subscribe_items as subscribe_workshop_items,
    subscribed_items as workshop_subscriptions, unsubscribe_item as unsubscribe_workshop_item,
    unsubscribe_items as unsubscribe_workshop_items,
};
pub use vapor_steam::{WorkshopItem, WorkshopPage, WorkshopSubscription};

pub use workspace::{
    BasicSourceWorkspaceReport, ContentDependencyDraft, SourceComposition, SourceCompositionLink,
    SourceContent, SourceMetadataRepairReport, SourceRuntimeOutputs, SourceWorkshopMetadata,
    SourceWorkshopPolicy, UnregisteredProjectCandidate, UnregisteredProjectScan,
    WorkshopPolicyDraft, adopt_workspace_content_project, create_basic_content_workspace,
    create_content_workspace, create_content_workspace_with_dependency_policies,
    create_content_workspace_with_workshop, discover_source_content, fork_acquired_content,
    initialize_source_workspace, load_source_composition, load_source_runtime_outputs,
    load_source_workshop_metadata, remember_workspace, repair_source_metadata,
    save_source_composition, save_source_runtime_outputs, save_source_workshop_metadata,
    scan_unregistered_content_projects, source_dependency_allowed, source_dependency_choice_group,
    source_dependency_relationship, source_required_dependency_choice_groups,
    unregister_workspace_content_project, validate_composition_source, validate_workspace,
    validate_workspace_source,
};
pub use workspace_files::{
    create_new_workspace_file, discover_workspace_files, read_workspace_file,
    read_workspace_file_with_fingerprint, workspace_file_fingerprint, write_workspace_file,
    write_workspace_file_if_fingerprint,
};

#[cfg(test)]
mod installation_role_snapshot_tests {
    use super::installation_role_snapshot_for;

    #[test]
    fn readiness_failure_does_not_erase_the_installed_role() {
        let snapshot = installation_role_snapshot_for(
            "ecosystem-developer",
            Err("packaged toolchain pin is missing".to_owned()),
        );

        assert_eq!(snapshot.role, "ecosystem-developer");
        assert!(!snapshot.rust_ready);
        assert_eq!(
            snapshot.error.as_deref(),
            Some("packaged toolchain pin is missing")
        );
    }
}
