//! Canonical content artifact and operation result vocabulary.
//!
//! Public fields remain encapsulated for external callers; provider operations use
//! the parent-module visibility to construct and consume these values.

use crate::manifest::ContentKind;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use vapor_content::authored_manifest::{AuthoredReference, AuthoredWorkshopPolicy};
use vapor_content::{ContentCandidates, RuntimePresentation, VaporId};

/// Source-discovered content artifact catalog.
#[derive(Debug, Clone)]
pub struct ContentCatalog {
    pub(super) artifacts: Vec<ContentArtifact>,
    pub(super) candidates: ContentCandidates,
}

impl ContentCatalog {
    /// Discovered publishable or installable source artifacts.
    pub fn artifacts(&self) -> &[ContentArtifact] {
        &self.artifacts
    }

    /// Find an artifact by full ID, local name, or published Workshop ID.
    pub fn find(&self, selector: &str) -> Option<&ContentArtifact> {
        if let Some((id, version)) = selector
            .rsplit_once('@')
            .filter(|(id, version)| !id.is_empty() && !version.is_empty())
        {
            return self.artifacts.iter().find(|artifact| {
                artifact.id == id && artifact.version.as_deref() == Some(version)
            });
        }
        self.artifacts.iter().find(|artifact| {
            artifact.id == selector
                || artifact.name == selector
                || artifact
                    .workshop
                    .published_file_id
                    .as_deref()
                    .is_some_and(|id| id == selector)
        })
    }

    pub(super) fn satisfying_version(
        &self,
        id: &str,
        requirement: Option<&semver::VersionReq>,
    ) -> Option<&ContentArtifact> {
        let id = id.parse::<VaporId>().ok()?;
        let candidate = match requirement {
            Some(requirement) => self.candidates.latest_matching(&id, requirement),
            None => self.candidates.latest(&id),
        };
        let Some(candidate) = candidate else {
            return requirement
                .is_none()
                .then(|| self.by_id(id.as_str()))
                .flatten();
        };
        let version = candidate.identity.version.to_string();
        self.artifacts.iter().find(|artifact| {
            artifact.id == id.as_str() && artifact.version.as_deref() == Some(version.as_str())
        })
    }

    pub(super) fn by_id(&self, id: &str) -> Option<&ContentArtifact> {
        self.artifacts.iter().find(|artifact| artifact.id == id)
    }
}

/// One source-authored Vapor content artifact.
#[derive(Debug, Clone)]
pub struct ContentArtifact {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) kind: ContentKind,
    pub(super) root: PathBuf,
    pub(super) manifest: PathBuf,
    pub(super) version: Option<String>,
    pub(super) binaries: Vec<String>,
    pub(super) libraries: Vec<String>,
    pub(super) runtime: Vec<RuntimePayload>,
    pub(super) presentation: RuntimePresentation,
    pub(super) dependencies: Vec<ContentReference>,
    pub(super) conflicts: Vec<ContentReference>,
    pub(super) workshop: WorkshopPolicy,
}

impl ContentArtifact {
    /// Fully-qualified content ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Local artifact name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Canonical content kind.
    pub fn kind(&self) -> ContentKind {
        self.kind
    }

    /// Source directory containing the artifact manifest.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Source artifact manifest path.
    pub fn manifest(&self) -> &Path {
        &self.manifest
    }

    /// Resolved artifact version, when declared.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Runtime executables copied from app-local Cargo output into `bin/`.
    pub fn binaries(&self) -> &[String] {
        &self.binaries
    }

    /// Runtime libraries copied from app-local Cargo output into `lib/`.
    pub fn libraries(&self) -> &[String] {
        &self.libraries
    }

    /// Host surface required by this content Engine.
    pub fn presentation(&self) -> RuntimePresentation {
        self.presentation
    }

    /// Target-specific deployed runtime payloads.
    pub(super) fn runtime(&self) -> &[RuntimePayload] {
        &self.runtime
    }

    /// Required or optional content dependencies and composition edges.
    pub fn dependencies(&self) -> &[ContentReference] {
        &self.dependencies
    }

    /// Declared content conflicts.
    pub fn conflicts(&self) -> &[ContentReference] {
        &self.conflicts
    }

    /// Authored Workshop publication policy.
    pub fn workshop(&self) -> &WorkshopPolicy {
        &self.workshop
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) struct RuntimePayload {
    pub(super) target: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) binaries: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) libraries: Vec<String>,
}

/// Authored dependency, conflict, or composition reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentReference {
    pub(super) id: String,
    #[serde(default = "default_reference_relationship")]
    pub(super) relationship: String,
    #[serde(default)]
    pub(super) optional: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) workshop_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) reason: Option<String>,
}

fn default_reference_relationship() -> String {
    "dependency".to_owned()
}

/// One editable edge in a source content dependency graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompositionLink {
    /// ID of the referenced content artifact.
    pub id: String,
    /// Role or relation assigned to the reference.
    pub relationship: String,
    /// Whether the composition remains valid when the reference is unavailable.
    #[serde(default)]
    pub optional: bool,
    /// Version constraint declared for the referenced artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Steam Workshop item ID used to acquire the artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workshop_id: Option<String>,
    /// Author-provided explanation for this relationship.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Source snapshot used by Studio's composition editor and optimistic save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceComposition {
    /// ID of the source content artifact.
    pub id: String,
    /// Authored artifact version, when present.
    pub version: Option<String>,
    /// Content kind declared by the manifest.
    #[serde(default)]
    pub kind: String,
    /// Path to the source manifest.
    pub manifest: PathBuf,
    /// Exact manifest text used for optimistic concurrency checks.
    pub source: String,
    /// Editable dependency, conflict, and composition links.
    pub links: Vec<CompositionLink>,
}

/// Snapshot of a source content manifest for structured Workshop metadata editing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceWorkshopMetadata {
    /// ID of the source content artifact.
    pub id: String,
    /// Authored artifact version, when present.
    pub version: Option<String>,
    /// Content kind declared by the manifest.
    pub kind: String,
    /// Path to the source manifest.
    pub manifest: PathBuf,
    /// Exact manifest text used for optimistic concurrency checks.
    pub source: String,
    /// Editable Steam Workshop policy values.
    pub policy: WorkshopPolicyDraft,
}

/// Snapshot of runtime executable and library declarations in a source manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRuntimeOutputs {
    /// ID of the source content artifact.
    pub id: String,
    /// Authored artifact version, when present.
    pub version: Option<String>,
    /// Content kind declared by the manifest.
    pub kind: String,
    /// Path to the source manifest.
    pub manifest: PathBuf,
    /// Exact manifest text used for optimistic concurrency checks.
    pub source: String,
    /// Declared executable names.
    pub binaries: Vec<String>,
    /// Declared shared-library names.
    pub libraries: Vec<String>,
}

impl From<&ContentReference> for CompositionLink {
    fn from(reference: &ContentReference) -> Self {
        Self {
            id: reference.id.to_string(),
            relationship: reference.relationship.clone(),
            optional: reference.optional,
            version: reference.version.clone(),
            workshop_id: reference.workshop_id.clone(),
            reason: reference.reason.clone(),
        }
    }
}

impl ContentReference {
    pub(super) fn from_authored(reference: &AuthoredReference, relationship: &str) -> Self {
        Self {
            id: reference.id.to_string(),
            relationship: relationship.to_owned(),
            optional: reference.optional,
            version: reference.version.as_ref().map(ToString::to_string),
            workshop_id: reference.workshop_id.clone(),
            reason: reference.reason.clone(),
        }
    }

    /// Referenced artifact ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Relationship label, such as `dependency`, `engine`, or `conflict`.
    pub fn relationship(&self) -> &str {
        &self.relationship
    }

    /// Whether missing referenced content is acceptable.
    pub fn optional(&self) -> bool {
        self.optional
    }

    /// Required version expression, when declared.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Referenced Workshop item ID, when declared.
    pub fn workshop_id(&self) -> Option<&str> {
        self.workshop_id.as_deref()
    }

    /// Human-readable reason, when declared.
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

/// Authored Workshop publication policy for one artifact.
pub type WorkshopPolicy = AuthoredWorkshopPolicy;

/// Optional Workshop publication metadata supplied when scaffolding content.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkshopPolicyDraft {
    /// Steam AppID that owns the target Workshop item.
    pub app_id: Option<u32>,
    /// Existing Workshop item ID, when updating an item.
    pub published_file_id: Option<String>,
    /// Requested Workshop visibility.
    pub visibility: Option<String>,
    /// Workshop item title.
    pub title: Option<String>,
    /// Workshop item description.
    pub description: Option<String>,
    /// Workshop tags applied to the item.
    pub tags: Vec<String>,
    /// Note shown with the next Workshop update.
    pub change_note: Option<String>,
}

impl WorkshopPolicyDraft {
    pub(super) fn is_empty(&self) -> bool {
        self.app_id.is_none()
            && self.published_file_id.is_none()
            && self.visibility.is_none()
            && self.title.is_none()
            && self.description.is_none()
            && self.tags.is_empty()
            && self.change_note.is_none()
    }
}

/// Stable tree fingerprint used by packages, locks, and receipts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Fingerprint {
    pub(super) algorithm: String,
    pub(super) hash: String,
    pub(super) files: u64,
    pub(super) bytes: u64,
}

impl Fingerprint {
    /// Fingerprint algorithm identifier.
    pub fn algorithm(&self) -> &str {
        &self.algorithm
    }

    /// Hex-encoded fingerprint hash.
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// Number of regular files included.
    pub fn files(&self) -> u64 {
        self.files
    }

    /// Number of file-content bytes included.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// Package staging result.
#[derive(Debug, Clone)]
pub struct PackageReport {
    pub(super) artifact_id: String,
    pub(super) root: PathBuf,
    pub(super) fingerprint: Fingerprint,
    pub(super) receipt: Option<PathBuf>,
    pub(super) dry_run: bool,
    pub(super) runtime_target: String,
}

impl PackageReport {
    /// Packaged artifact ID.
    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }

    /// Staged artifact root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Staged artifact fingerprint.
    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// Operation receipt path, absent for dry-runs.
    pub fn receipt(&self) -> Option<&Path> {
        self.receipt.as_deref()
    }

    /// Whether the package was only previewed.
    pub fn dry_run(&self) -> bool {
        self.dry_run
    }

    /// Runtime target triple used for copied binaries and libraries.
    pub fn runtime_target(&self) -> &str {
        &self.runtime_target
    }
}

/// Validation result for one or more source artifacts.
#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub(super) checked: Vec<String>,
    pub(super) diagnostics: Vec<String>,
}

impl ValidationReport {
    /// Artifact IDs that were checked.
    pub fn checked(&self) -> &[String] {
        &self.checked
    }

    /// Non-fatal validation diagnostics.
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }
}

/// Result of acquiring content into the app-owned cache.
#[derive(Debug, Clone)]
pub struct AcquireReport {
    pub(super) artifact_id: String,
    pub(super) cache_root: PathBuf,
    pub(super) fingerprint: Fingerprint,
    pub(super) receipt: PathBuf,
}

impl AcquireReport {
    /// Acquired artifact ID.
    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }

    /// Cache root containing the package.
    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }

    /// Cached artifact-root fingerprint.
    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// Operation receipt path.
    pub fn receipt(&self) -> &Path {
        &self.receipt
    }
}

/// Result of installing or updating content.
#[derive(Debug, Clone)]
pub struct InstallReport {
    pub(super) artifact_id: String,
    pub(super) version: Option<String>,
    pub(super) installed_root: PathBuf,
    pub(super) fingerprint: Fingerprint,
    pub(super) receipt: PathBuf,
}

impl InstallReport {
    /// Installed artifact ID.
    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }

    /// Installed exact content version, when declared.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Installed artifact root.
    pub fn installed_root(&self) -> &Path {
        &self.installed_root
    }

    /// Installed artifact-root fingerprint.
    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// Operation receipt path.
    pub fn receipt(&self) -> &Path {
        &self.receipt
    }
}

/// Result of uninstalling content.
#[derive(Debug, Clone)]
pub struct UninstallReport {
    pub(super) artifact_id: String,
    pub(super) removed: bool,
    pub(super) receipt: PathBuf,
}

impl UninstallReport {
    /// Uninstalled artifact ID.
    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }

    /// Whether an installed or disabled artifact root was removed.
    pub fn removed(&self) -> bool {
        self.removed
    }

    /// Operation receipt path.
    pub fn receipt(&self) -> &Path {
        &self.receipt
    }
}

/// Selected installed packagepack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackagepackSelection {
    pub(super) schema_version: u32,
    pub(super) artifact_id: String,
    pub(super) installed_root: PathBuf,
    pub(super) fingerprint: Fingerprint,
    pub(super) selected_at: u64,
}

impl PackagepackSelection {
    /// Selected packagepack artifact ID.
    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }

    /// Installed artifact root selected for play.
    pub fn installed_root(&self) -> &Path {
        &self.installed_root
    }

    /// Fingerprint selected at the time of selection.
    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }
}

/// Verification result for one installed artifact.
#[derive(Debug, Clone)]
pub struct VerifyReport {
    pub(super) artifact_id: String,
    pub(super) ok: bool,
    pub(super) expected: Option<Fingerprint>,
    pub(super) observed: Option<Fingerprint>,
    pub(super) detail: String,
}

impl VerifyReport {
    /// Verified artifact ID.
    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }

    /// Whether the installed artifact root matches its receipt/index fingerprint.
    pub fn ok(&self) -> bool {
        self.ok
    }

    /// Expected fingerprint from app-owned state.
    pub fn expected(&self) -> Option<&Fingerprint> {
        self.expected.as_ref()
    }

    /// Observed fingerprint from the installed artifact root.
    pub fn observed(&self) -> Option<&Fingerprint> {
        self.observed.as_ref()
    }

    /// Diagnostic detail.
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// Publish, create, or delete preview/execution result.
#[derive(Debug, Clone)]
pub struct WorkshopOperationReport {
    pub(super) artifact_id: String,
    pub(super) script: Option<PathBuf>,
    pub(super) receipt: PathBuf,
    pub(super) uploaded: bool,
    pub(super) published_file_id: Option<String>,
}

impl WorkshopOperationReport {
    /// Target artifact or Workshop ID.
    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }

    /// Generated provider script, when one exists.
    pub fn script(&self) -> Option<&Path> {
        self.script.as_deref()
    }

    /// Operation receipt path.
    pub fn receipt(&self) -> &Path {
        &self.receipt
    }

    /// Whether a provider upload was attempted and accepted.
    pub fn uploaded(&self) -> bool {
        self.uploaded
    }

    /// PublishedFileId created or used by the provider, when known.
    pub fn published_file_id(&self) -> Option<&str> {
        self.published_file_id.as_deref()
    }
}
