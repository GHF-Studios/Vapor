use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use vapor_content::{
    ContentCandidate, ContentKind, ContentVersionId, DependencySpec, resolve_report_from,
    validate_content_graph,
};

/// Provider outcome for one selected node in an install preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallPlanSource {
    /// Authored content in the selected source workspace.
    Workspace,
    /// Content already downloaded into Vapor's app-owned library.
    Cache,
    /// An enabled artifact already present in the app-owned installation.
    Installed,
    /// A first-party Workshop seed that must be acquired before resolution.
    RootSeed,
}

/// One required or optional edge in an install preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallPlanDependency {
    /// Referenced stable content identity.
    pub id: String,
    /// Authored relationship such as `engine`, `game`, or `dependency`.
    pub relationship: String,
    /// Accepted version expression, when specified.
    pub version: Option<String>,
    /// Optional edges do not block installation when unavailable.
    pub optional: bool,
}

/// One selected node and its observed availability for an install operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallPlanNode {
    /// Stable content identity.
    pub id: String,
    /// Exact selected content version, when declared.
    pub version: Option<String>,
    /// Rust-defined Vapor content kind.
    pub kind: String,
    /// Workshop PublishedFileId, when known.
    pub published_file_id: Option<String>,
    /// Provider outcome or first-party acquisition requirement.
    pub source: InstallPlanSource,
    /// Whether this exact node is already enabled in the installation.
    pub already_installed: bool,
    /// Declared child edges from this node's manifest.
    pub dependencies: Vec<InstallPlanDependency>,
}

/// Read-only application plan for materializing a composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallPlan {
    /// Selector requested by the user.
    pub selector: String,
    /// Resolved nodes in dependency-first order.
    pub nodes: Vec<InstallPlanNode>,
    /// Conditions that must be addressed before the operation can proceed.
    pub blockers: Vec<String>,
    /// Non-blocking notes such as omitted optional content.
    pub warnings: Vec<String>,
}

impl InstallPlan {
    /// Whether every required dependency is locally resolvable.
    pub fn ready(&self) -> bool {
        self.blockers.is_empty()
    }
}

/// Provider result for one root or dependency selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallCandidateAvailability {
    /// One exact provider candidate is available.
    Selected(SelectedInstallCandidate),
    /// No candidate is available yet; the provider may attach recovery details.
    Unavailable {
        /// Human-readable reason, when the host can provide a specific one.
        message: Option<String>,
        /// Whether the absence is non-blocking.
        warning: bool,
        /// A Workshop seed that can be acquired to make this candidate inspectable.
        seed: Option<InstallPlanNode>,
    },
}

/// Exact semantic candidate plus the provider facts needed in an install plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedInstallCandidate {
    /// Provider-neutral exact candidate used by the resolver.
    pub candidate: ContentCandidate,
    /// Availability and authored-edge projection shown for this node.
    pub plan_node: InstallPlanNode,
    /// Content IDs this artifact conflicts with.
    pub conflicts: Vec<String>,
    /// Conflicting IDs that are currently enabled in the installation.
    pub enabled_conflicts: Vec<String>,
}

/// Host operations required by the shared install planner.
///
/// Providers choose exact candidates and report local availability. They do
/// not walk the dependency graph, validate compatibility, or decide plan
/// readiness.
pub trait InstallContentProvider {
    /// Select the requested root from eligible local sources.
    fn select_root(&mut self, selector: &str) -> Result<InstallCandidateAvailability, String>;

    /// Select one dependency candidate using the authored requirement and the
    /// selected parent's provider outcome.
    fn select_dependency(
        &mut self,
        parent: &ContentVersionId,
        parent_source: InstallPlanSource,
        binding: &str,
        requirement: &DependencySpec,
    ) -> Result<InstallCandidateAvailability, String>;
}

/// Resolve a playable composition through a provider while keeping all graph
/// traversal, exact-edge validation, compatibility, and plan readiness shared.
pub fn resolve_install_plan(
    selector: &str,
    provider: &mut impl InstallContentProvider,
) -> Result<InstallPlan, String> {
    let mut builder = InstallPlanBuilder::default();
    let root = match provider.select_root(selector)? {
        InstallCandidateAvailability::Selected(candidate) => builder
            .retain(candidate)?
            .expect("selected provider result has an exact candidate"),
        InstallCandidateAvailability::Unavailable {
            message,
            warning,
            seed,
        } => {
            builder.record_unavailable(None, None, message, warning, seed);
            return Ok(builder.finish(selector));
        }
    };
    let root_kind = root.kind;
    let mut provider_error = None;
    let mut report = resolve_report_from(root, |parent, binding, dependency| {
        let Some(parent_record) = builder.candidates.get(&parent.identity) else {
            provider_error = Some(format!(
                "resolved parent '{}' has no retained provider record",
                parent.identity
            ));
            return None;
        };
        match provider.select_dependency(
            &parent.identity,
            parent_record.plan_node.source,
            binding,
            dependency,
        ) {
            Ok(InstallCandidateAvailability::Selected(candidate)) => {
                match builder.retain(candidate) {
                    Ok(candidate) => candidate,
                    Err(error) => {
                        provider_error = Some(error);
                        None
                    }
                }
            }
            Ok(InstallCandidateAvailability::Unavailable {
                message,
                warning,
                seed,
            }) => {
                builder.record_unavailable(
                    Some(&parent.identity),
                    Some(binding),
                    message,
                    warning,
                    seed,
                );
                None
            }
            Err(error) => {
                provider_error = Some(error);
                None
            }
        }
    });
    if let Some(error) = provider_error {
        return Err(error);
    }

    let graph_complete = report.is_complete();
    for issue in report.issues.drain(..) {
        match &issue {
            vapor_content::ContentResolutionError::DependencyCycle { .. } => {
                push_unique(&mut builder.blockers, issue.to_string());
            }
            vapor_content::ContentResolutionError::DependencyNotFound {
                depender, binding, ..
            } if !builder
                .diagnosed_missing
                .contains(&(depender.clone(), binding.clone())) =>
            {
                push_unique(&mut builder.blockers, issue.to_string());
            }
            _ => {}
        }
    }

    if root_kind != ContentKind::Packagepack {
        push_unique(
            &mut builder.blockers,
            format!(
                "A launchable composition must be rooted in a Packagepack; '{}' is a {}",
                report.graph.root.id, root_kind,
            ),
        );
    } else if graph_complete
        && builder.blockers.is_empty()
        && let Err(error) = validate_content_graph(&report.graph)
    {
        push_unique(&mut builder.blockers, error.to_string());
    }

    let selected_ids: BTreeSet<_> = report
        .graph
        .nodes
        .keys()
        .map(|identity| identity.id.to_string())
        .collect();
    let mut versions = BTreeMap::new();
    for resolved in report.graph.dependency_first() {
        let Some(candidate) = builder.candidates.get(&resolved.identity) else {
            continue;
        };
        if let Some(previous) = versions.insert(
            resolved.identity.id.to_string(),
            resolved.identity.version.to_string(),
        ) && previous != resolved.identity.version.to_string()
        {
            push_unique(
                &mut builder.blockers,
                format!(
                    "composition resolves multiple versions of '{}', but installation stores one version per content ID",
                    resolved.identity.id,
                ),
            );
        }
        for conflict in &candidate.plan_node.dependencies {
            if conflict.optional {
                push_unique(
                    &mut builder.warnings,
                    format!(
                        "optional {} '{}' is not selected automatically",
                        conflict.relationship, conflict.id
                    ),
                );
            }
        }
        for conflict in &candidate.conflicts {
            if selected_ids.contains(conflict) {
                push_unique(
                    &mut builder.blockers,
                    format!(
                        "{} conflicts with planned content '{}'",
                        candidate.plan_node.id, conflict
                    ),
                );
            }
        }
        for conflict in &candidate.enabled_conflicts {
            push_unique(
                &mut builder.blockers,
                format!(
                    "{} conflicts with installed content '{}'",
                    candidate.plan_node.id, conflict
                ),
            );
        }
        builder.nodes.push(candidate.plan_node.clone());
    }
    Ok(builder.finish(selector))
}

#[derive(Default)]
struct InstallPlanBuilder {
    candidates: BTreeMap<ContentVersionId, SelectedInstallCandidate>,
    nodes: Vec<InstallPlanNode>,
    blockers: Vec<String>,
    warnings: Vec<String>,
    diagnosed_missing: BTreeSet<(ContentVersionId, String)>,
}

impl InstallPlanBuilder {
    fn retain(
        &mut self,
        candidate: SelectedInstallCandidate,
    ) -> Result<Option<ContentCandidate>, String> {
        let identity = candidate.candidate.identity.clone();
        let expected_id = identity.id.to_string();
        let expected_version = identity.version.to_string();
        if candidate.plan_node.id != expected_id
            || candidate.plan_node.version.as_deref() != Some(expected_version.as_str())
        {
            return Err(format!(
                "provider plan facts do not match selected identity '{identity}'"
            ));
        }
        if let Some(existing) = self.candidates.get(&identity) {
            return Ok(Some(existing.candidate.clone()));
        }
        self.candidates.insert(identity, candidate.clone());
        Ok(Some(candidate.candidate))
    }

    fn record_unavailable(
        &mut self,
        parent: Option<&ContentVersionId>,
        binding: Option<&str>,
        message: Option<String>,
        warning: bool,
        seed: Option<InstallPlanNode>,
    ) {
        if let Some(seed) = seed
            && !self.nodes.contains(&seed)
        {
            self.nodes.push(seed);
        }
        if let Some(message) = message {
            let messages = if warning {
                &mut self.warnings
            } else {
                &mut self.blockers
            };
            push_unique(messages, message);
            if let (Some(parent), Some(binding)) = (parent, binding) {
                self.diagnosed_missing
                    .insert((parent.clone(), binding.to_owned()));
            }
        }
    }

    fn finish(self, selector: &str) -> InstallPlan {
        InstallPlan {
            selector: selector.to_owned(),
            nodes: self.nodes,
            blockers: self.blockers,
            warnings: self.warnings,
        }
    }
}

fn push_unique(messages: &mut Vec<String>, message: String) {
    if !messages.contains(&message) {
        messages.push(message);
    }
}
