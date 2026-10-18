//! Workshop/content discovery, packaging, installation state, and receipts.
//!
//! The module keeps SteamUGC and filesystem staging behind Vapor content
//! operations. Authored intent is read from source role-specific manifests; generated
//! app-owned state is written under the Steam installation/app root.

use crate::{
    cross_toolchain,
    discovery::{EnvironmentPaths, InstallationPaths, ensure_contained},
    manifest::{self, ContentKind, VaporEntity},
    steam,
    workspace::WorkspaceManifest,
};
use vapor_content::{ContentCandidate, ContentCandidates, ContentVersionId, DependencySpec, VaporId};
pub use vapor_client_domain::{
    InstallCandidateAvailability, InstallPlan, InstallPlanDependency, InstallPlanNode,
    InstallPlanSource, SelectedInstallCandidate,
};
use vapor_content::authored_manifest::{
    AuthoredContentManifest, AuthoredVersion, AuthoredWorkshopPolicy, AuthoredReference,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

const CONTENT_STATE_SCHEMA: u32 = 1;
const FINGERPRINT_ALGORITHM: &str = "vapor-fnv1a-64-v1";

/// Canonical app-root layout for generated content state.
#[derive(Debug, Clone)]
pub struct ContentLayout {
    app_content: PathBuf,
    steam_downloads: PathBuf,
    cache: PathBuf,
    installed: PathBuf,
    disabled: PathBuf,
    quarantine: PathBuf,
    output_packages: PathBuf,
    output_scripts: PathBuf,
    state: PathBuf,
    locks: PathBuf,
    receipts: PathBuf,
    selection: PathBuf,
    index: PathBuf,
}

impl ContentLayout {
    /// Build layout paths under the Steam installation/app root.
    pub fn new(installation: &InstallationPaths) -> Self {
        let app_content = installation.root().join("content");
        let state = installation.state_dir().join("content");
        Self {
            steam_downloads: app_content.join("workshop/downloads"),
            cache: app_content.join("cache"),
            installed: app_content.join("installed"),
            disabled: app_content.join("disabled"),
            quarantine: app_content.join("quarantine"),
            output_packages: installation.root().join("output/content/packages"),
            output_scripts: installation.root().join("output/content/scripts"),
            app_content,
            locks: state.join("locks"),
            receipts: state.join("receipts"),
            selection: state.join("selection.toml"),
            index: state.join("index.toml"),
            state,
        }
    }

    /// Runtime content root.
    pub fn app_content(&self) -> &Path {
        &self.app_content
    }

    /// Provider-observed Steam Workshop download records.
    pub fn steam_downloads(&self) -> &Path {
        &self.steam_downloads
    }

    /// Vapor-managed package cache root.
    pub fn cache(&self) -> &Path {
        &self.cache
    }

    /// Active installed content root.
    pub fn installed(&self) -> &Path {
        &self.installed
    }

    /// Disabled but retained content root.
    pub fn disabled(&self) -> &Path {
        &self.disabled
    }

    /// Corrupted or incomplete content quarantine root.
    pub fn quarantine(&self) -> &Path {
        &self.quarantine
    }

    /// Content package staging output.
    pub fn output_packages(&self) -> &Path {
        &self.output_packages
    }

    /// Steam/Workshop script preview output.
    pub fn output_scripts(&self) -> &Path {
        &self.output_scripts
    }

    /// Generated content state root.
    pub fn state(&self) -> &Path {
        &self.state
    }

    /// Generated resolution lock directory.
    pub fn locks(&self) -> &Path {
        &self.locks
    }

    /// Operation receipt directory.
    pub fn receipts(&self) -> &Path {
        &self.receipts
    }

    /// Selected packagepack state path.
    pub fn selection(&self) -> &Path {
        &self.selection
    }

    /// Installed-content index path.
    pub fn index(&self) -> &Path {
        &self.index
    }
}

/// Source-discovered content artifact catalog.
#[derive(Debug, Clone)]
pub struct ContentCatalog {
    artifacts: Vec<ContentArtifact>,
    candidates: ContentCandidates,
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

    fn satisfying_version(
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

    fn by_id(&self, id: &str) -> Option<&ContentArtifact> {
        self.artifacts.iter().find(|artifact| artifact.id == id)
    }
}

/// One source-authored Vapor content artifact.
#[derive(Debug, Clone)]
pub struct ContentArtifact {
    id: String,
    name: String,
    kind: ContentKind,
    root: PathBuf,
    manifest: PathBuf,
    version: Option<String>,
    binaries: Vec<String>,
    libraries: Vec<String>,
    runtime: Vec<RuntimePayload>,
    dependencies: Vec<ContentReference>,
    conflicts: Vec<ContentReference>,
    workshop: WorkshopPolicy,
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

    /// Target-specific deployed runtime payloads.
    fn runtime(&self) -> &[RuntimePayload] {
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
struct RuntimePayload {
    target: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    binaries: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    libraries: Vec<String>,
}

/// Authored dependency, conflict, or composition reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentReference {
    id: String,
    #[serde(default = "default_reference_relationship")]
    relationship: String,
    #[serde(default)]
    optional: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workshop_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
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
    fn from_authored(
        reference: &AuthoredReference,
        relationship: &str,
    ) -> Self {
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
    fn is_empty(&self) -> bool {
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
    algorithm: String,
    hash: String,
    files: u64,
    bytes: u64,
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
    artifact_id: String,
    root: PathBuf,
    fingerprint: Fingerprint,
    receipt: Option<PathBuf>,
    dry_run: bool,
    runtime_target: String,
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
    checked: Vec<String>,
    diagnostics: Vec<String>,
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
    artifact_id: String,
    cache_root: PathBuf,
    fingerprint: Fingerprint,
    receipt: PathBuf,
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
    artifact_id: String,
    version: Option<String>,
    installed_root: PathBuf,
    fingerprint: Fingerprint,
    receipt: PathBuf,
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
    artifact_id: String,
    removed: bool,
    receipt: PathBuf,
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
    schema_version: u32,
    artifact_id: String,
    installed_root: PathBuf,
    fingerprint: Fingerprint,
    selected_at: u64,
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

/// Installed Packagepack graph resolved by the Vapor local provider.
#[derive(Debug, Clone)]
pub struct InstalledPackagepackGraph {
    root: InstalledArtifactSnapshot,
    engine: InstalledArtifactSnapshot,
    game: InstalledArtifactSnapshot,
    nodes: Vec<InstalledArtifactSnapshot>,
}

impl InstalledPackagepackGraph {
    /// Exact installed Packagepack root.
    pub fn root(&self) -> &InstalledArtifactSnapshot {
        &self.root
    }

    /// Effective installed Engine.
    pub fn engine(&self) -> &InstalledArtifactSnapshot {
        &self.engine
    }

    /// Effective installed Game.
    pub fn game(&self) -> &InstalledArtifactSnapshot {
        &self.game
    }

    /// Required resolved nodes, including root, Engine, Game, and dependencies.
    pub fn nodes(&self) -> &[InstalledArtifactSnapshot] {
        &self.nodes
    }
}

/// Fully resolved, verified plan for launching one installed composition.
#[derive(Debug, Clone)]
pub struct PackagepackLaunchPlan {
    packagepack_id: String,
    packagepack_root: PathBuf,
    engine_id: String,
    engine_root: PathBuf,
    game_id: String,
    game_root: PathBuf,
    runtime_target: String,
    binary: PathBuf,
    verified_ids: Vec<String>,
}

impl PackagepackLaunchPlan {
    /// Selected Packagepack content ID.
    pub fn packagepack_id(&self) -> &str {
        &self.packagepack_id
    }

    /// Installed Packagepack root.
    pub fn packagepack_root(&self) -> &Path {
        &self.packagepack_root
    }

    /// Required Engine content ID.
    pub fn engine_id(&self) -> &str {
        &self.engine_id
    }

    /// Installed Engine root.
    pub fn engine_root(&self) -> &Path {
        &self.engine_root
    }

    /// Required Game content ID.
    pub fn game_id(&self) -> &str {
        &self.game_id
    }

    /// Installed Game root.
    pub fn game_root(&self) -> &Path {
        &self.game_root
    }

    /// Host runtime target selected for the Packagepack executable.
    pub fn runtime_target(&self) -> &str {
        &self.runtime_target
    }

    /// Verified Engine binary path.
    pub fn binary(&self) -> &Path {
        &self.binary
    }

    /// Content IDs whose installed closure was checked.
    pub fn verified_ids(&self) -> &[String] {
        &self.verified_ids
    }
}

/// Resolve and verify an installed Packagepack without changing global selection.
///
/// # Errors
///
/// Returns missing, disabled, incompatible, corrupt, or invalid composition errors.
pub fn installed_packagepack_graph(
    installation: &InstallationPaths,
    selector: &str,
) -> Result<InstalledPackagepackGraph, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    let packagepack_id = resolve_installed_selector(&index, selector)?;
    inspect_installed_packagepack(&index, &packagepack_id)
}

fn inspect_installed_packagepack(
    index: &ContentIndex,
    packagepack_id: &str,
) -> Result<InstalledPackagepackGraph, String> {
    let packagepack = index
        .installed
        .get(packagepack_id)
        .ok_or_else(|| format!("Packagepack is not installed: {packagepack_id}"))?;
    if packagepack.kind != ContentKind::Packagepack.to_string() {
        return Err(format!(
            "{} is {}, not a Packagepack",
            packagepack_id, packagepack.kind
        ));
    }
    let mut visiting = BTreeSet::new();
    let mut verified = BTreeSet::new();
    verify_launch_closure(index, packagepack_id, &mut visiting, &mut verified)?;

    let dependencies = launch_dependencies(&packagepack.installed_root)?;
    let engine_id = resolve_composed_role(
        index,
        &dependencies,
        "engine",
        "enginepack",
        "engine",
        ContentKind::Enginepack,
        packagepack_id,
    )?
    .ok_or_else(|| {
        format!(
            "Packagepack '{}' has no required Engine or Enginepack dependency",
            packagepack_id
        )
    })?;
    let engine = index.installed.get(&engine_id).ok_or_else(|| {
        format!(
            "Packagepack '{}' requires missing Engine '{}'",
            packagepack_id, engine_id
        )
    })?;
    if engine.kind != ContentKind::Engine.to_string() {
        return Err(format!(
            "Packagepack '{}' resolves Engine '{}' to content kind {}",
            packagepack_id, engine_id, engine.kind
        ));
    }
    let game_id = resolve_composed_role(
        index,
        &dependencies,
        "game",
        "gamepack",
        "game",
        ContentKind::Gamepack,
        packagepack_id,
    )?
    .ok_or_else(|| {
        format!(
            "Packagepack '{}' has no required Game or Gamepack dependency",
            packagepack_id
        )
    })?;
    let game = index.installed.get(&game_id).ok_or_else(|| {
        format!(
            "Packagepack '{}' requires missing Game '{}'",
            packagepack_id, game_id
        )
    })?;
    if game.kind != ContentKind::Game.to_string() {
        return Err(format!(
            "Packagepack '{}' resolves Game '{}' to content kind {}",
            packagepack_id, game_id, game.kind
        ));
    }
    let game = installed_snapshot_from_record(game);
    let nodes = verified
        .iter()
        .filter_map(|id| index.installed.get(id))
        .map(installed_snapshot_from_record)
        .collect();
    Ok(InstalledPackagepackGraph {
        root: installed_snapshot_from_record(packagepack),
        engine: installed_snapshot_from_record(engine),
        game,
        nodes,
    })
}

/// Resolve and verify an installed Packagepack without changing global selection.
///
/// # Errors
///
/// Returns missing, disabled, incompatible, corrupt, or invalid composition errors.
pub fn prepare_packagepack_launch(
    installation: &InstallationPaths,
    selector: &str,
) -> Result<PackagepackLaunchPlan, String> {
    let graph = installed_packagepack_graph(installation, selector)?;
    let runtime_target = host_runtime_target();
    let binary = packagepack_launch_binary(graph.root.installed_root(), &runtime_target)?;
    Ok(PackagepackLaunchPlan {
        packagepack_id: graph.root.artifact_id().to_owned(),
        packagepack_root: graph.root.installed_root().to_path_buf(),
        engine_id: graph.engine.artifact_id().to_owned(),
        engine_root: graph.engine.installed_root().to_path_buf(),
        game_id: graph.game.artifact_id().to_owned(),
        game_root: graph.game.installed_root().to_path_buf(),
        runtime_target,
        binary,
        verified_ids: graph
            .nodes
            .iter()
            .map(|node| node.artifact_id().to_owned())
            .collect(),
    })
}

/// Run a previously resolved installed composition.
///
/// # Errors
///
/// Returns process start or nonzero exit errors.
pub fn run_packagepack_launch(
    plan: &PackagepackLaunchPlan,
    launch_request: &str,
    arguments: &[String],
) -> Result<(), String> {
    let mut command = Command::new(&plan.binary);
    command
        .current_dir(&plan.packagepack_root)
        .env("VAPOR_LAUNCH_REQUEST", launch_request)
        .env("VAPOR_PACKAGEPACK_ID", &plan.packagepack_id)
        .env("VAPOR_PACKAGEPACK_ROOT", &plan.packagepack_root)
        .env("VAPOR_ENGINE_ID", &plan.engine_id)
        .env("VAPOR_ENGINE_ROOT", &plan.engine_root)
        .env("VAPOR_RUNTIME_TARGET", &plan.runtime_target)
        .args(arguments);
    command
        .env("VAPOR_GAME_ID", &plan.game_id)
        .env("VAPOR_GAME_ROOT", &plan.game_root);
    let status = command
        .status()
        .map_err(|error| format!("failed to launch '{}': {error}", plan.binary.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "engine content {} exited with {status}",
            plan.engine_id
        ))
    }
}

fn verify_launch_closure(
    index: &ContentIndex,
    artifact_id: &str,
    visiting: &mut BTreeSet<String>,
    verified: &mut BTreeSet<String>,
) -> Result<(), String> {
    if verified.contains(artifact_id) {
        return Ok(());
    }
    if !visiting.insert(artifact_id.to_owned()) {
        return Err(format!(
            "installed content dependency cycle includes {artifact_id}"
        ));
    }
    let result = (|| {
        let record = index
            .installed
            .get(artifact_id)
            .ok_or_else(|| format!("required content is not installed: {artifact_id}"))?;
        if !record.enabled {
            return Err(format!("required content is disabled: {artifact_id}"));
        }
        let report = verify_one(index, artifact_id);
        if !report.ok() {
            return Err(format!(
                "installed content needs repair: {artifact_id}: {}",
                report.detail()
            ));
        }
        let artifact = read_deployed_manifest(&record.installed_root)?;
        validate_artifact_dependency_roles(&artifact)?;
        for conflict in &record.conflicts {
            if index
                .installed
                .get(conflict.id())
                .is_some_and(|item| item.enabled)
            {
                return Err(format!(
                    "{artifact_id} conflicts with installed content '{}'",
                    conflict.id()
                ));
            }
        }
        for dependency in artifact.dependencies() {
            if dependency.optional() && !index.installed.contains_key(dependency.id()) {
                continue;
            }
            let requirement = dependency
                .version()
                .map(semver::VersionReq::parse)
                .transpose()
                .map_err(|error| {
                    format!(
                        "{artifact_id} has invalid version requirement for {}: {error}",
                        dependency.id()
                    )
                })?;
            let child = index.installed.get(dependency.id()).ok_or_else(|| {
                format!(
                    "{artifact_id} requires missing {} '{}'",
                    dependency.relationship(),
                    dependency.id()
                )
            })?;
            if !version_satisfies(child.version.as_deref(), requirement.as_ref()) {
                return Err(format!(
                    "{artifact_id} requires {} '{}' at {}, but installed version is {}",
                    dependency.relationship(),
                    dependency.id(),
                    dependency.version().unwrap_or("any version"),
                    child.version.as_deref().unwrap_or("unversioned")
                ));
            }
            let child_artifact = read_deployed_manifest(&child.installed_root)?;
            validate_dependency_edge(artifact.kind(), dependency.relationship(), &child_artifact)?;
            verify_launch_closure(index, dependency.id(), visiting, verified)?;
        }
        Ok(())
    })();
    visiting.remove(artifact_id);
    if result.is_ok() {
        verified.insert(artifact_id.to_owned());
    }
    result
}

fn launch_dependencies(packagepack_root: &Path) -> Result<Vec<LaunchReference>, String> {
    launch_role_dependencies(
        packagepack_root,
        manifest::PACKAGEPACK_FILE_NAME,
        "packagepack",
    )
}

fn launch_role_dependencies(
    content_root: &Path,
    manifest_name: &str,
    role: &str,
) -> Result<Vec<LaunchReference>, String> {
    let manifest = content_root.join(manifest_name);
    let source = fs::read_to_string(&manifest)
        .map_err(|error| format!("failed to read '{}': {error}", manifest.display()))?;
    let parsed: LaunchManifest = toml::from_str(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", manifest.display()))?;
    let content = match role {
        "packagepack" => parsed.packagepack,
        "enginepack" => parsed.enginepack,
        "gamepack" => parsed.gamepack,
        _ => None,
    }
    .ok_or_else(|| {
        format!(
            "selected content manifest '{}' has no [{role}] section",
            manifest.display()
        )
    })?;
    Ok(content.dependencies)
}

fn resolve_composed_role(
    index: &ContentIndex,
    dependencies: &[LaunchReference],
    direct_relationship: &str,
    pack_relationship: &str,
    member_relationship: &str,
    pack_kind: ContentKind,
    packagepack_id: &str,
) -> Result<Option<String>, String> {
    let matching_ids = |relationship: &str| {
        dependencies
            .iter()
            .filter(|dependency| dependency.relationship == relationship)
            .map(|dependency| dependency.id.clone())
            .collect::<Vec<_>>()
    };
    let direct_ids = matching_ids(direct_relationship);
    let pack_ids = matching_ids(pack_relationship);
    if direct_ids.len() > 1 || pack_ids.len() > 1 {
        return Err(format!(
            "Packagepack '{packagepack_id}' declares more than one {direct_relationship} or {pack_relationship}"
        ));
    }
    let direct = direct_ids.into_iter().next();
    let pack = pack_ids.into_iter().next();
    if direct.is_some() && pack.is_some() {
        return Err(format!(
            "Packagepack '{packagepack_id}' declares both {direct_relationship} and {pack_relationship}"
        ));
    }
    if let Some(id) = direct {
        return Ok(Some(id));
    }
    let Some(pack_id) = pack else {
        return Ok(None);
    };
    let record = index.installed.get(&pack_id).ok_or_else(|| {
        format!("Packagepack '{packagepack_id}' requires missing {pack_relationship} '{pack_id}'")
    })?;
    if record.kind != pack_kind.to_string() {
        return Err(format!(
            "Packagepack '{packagepack_id}' resolves {pack_relationship} '{pack_id}' to content kind {}",
            record.kind
        ));
    }
    let (manifest_name, role) = match pack_kind {
        ContentKind::Enginepack => (manifest::ENGINEPACK_FILE_NAME, "enginepack"),
        ContentKind::Gamepack => (manifest::GAMEPACK_FILE_NAME, "gamepack"),
        _ => return Err(format!("unsupported composition pack kind: {pack_kind}")),
    };
    let members = launch_role_dependencies(&record.installed_root, manifest_name, role)?
        .into_iter()
        .filter(|dependency| dependency.relationship == member_relationship)
        .map(|dependency| dependency.id)
        .collect::<Vec<_>>();
    match members.as_slice() {
        [member] => Ok(Some(member.clone())),
        [] => Err(format!(
            "{pack_relationship} '{pack_id}' has no required {member_relationship} dependency"
        )),
        _ => Err(format!(
            "{pack_relationship} '{pack_id}' has more than one {member_relationship} dependency"
        )),
    }
}

fn packagepack_launch_binary(
    packagepack_root: &Path,
    runtime_target: &str,
) -> Result<PathBuf, String> {
    let manifest_path = packagepack_root.join(manifest::PACKAGEPACK_FILE_NAME);
    let source = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("failed to read '{}': {error}", manifest_path.display()))?;
    let parsed: LaunchManifest = toml::from_str(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", manifest_path.display()))?;
    let packagepack = parsed.packagepack.ok_or_else(|| {
        format!(
            "resolved Packagepack manifest '{}' has no [packagepack] section",
            manifest_path.display()
        )
    })?;
    let runtime = packagepack
        .runtime
        .iter()
        .find(|runtime| runtime.target == runtime_target)
        .ok_or_else(|| {
            format!(
                "Packagepack '{}' has no ready-to-run build for {runtime_target}; update this game setup to download its executable",
                manifest_path.display()
            )
        })?;
    let binary_name = runtime.binaries.first().ok_or_else(|| {
        format!(
            "Packagepack '{}' contains no executable for {runtime_target}; publish a built Packagepack before launching",
            manifest_path.display()
        )
    })?;
    let mut components = Path::new(binary_name).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(format!(
            "Packagepack runtime binary must be a file name, got '{binary_name}'"
        ));
    }
    let binary = packagepack_root
        .join("bin")
        .join(runtime_target)
        .join(binary_name);
    let metadata = fs::symlink_metadata(&binary).map_err(|error| {
        format!(
            "resolved Packagepack executable is missing: {}: {error}",
            binary.display()
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "resolved Packagepack executable is not a regular file: {}",
            binary.display()
        ));
    }
    Ok(binary)
}

#[derive(Debug, Deserialize)]
struct LaunchManifest {
    packagepack: Option<LaunchContent>,
    enginepack: Option<LaunchContent>,
    gamepack: Option<LaunchContent>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct LaunchContent {
    #[serde(default)]
    runtime: Vec<LaunchRuntime>,
    #[serde(default)]
    dependencies: Vec<LaunchReference>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct LaunchRuntime {
    target: String,
    #[serde(default)]
    binaries: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct LaunchReference {
    id: String,
    #[serde(default = "default_launch_relationship")]
    relationship: String,
}

fn default_launch_relationship() -> String {
    "dependency".to_owned()
}

/// Verification result for one installed artifact.
#[derive(Debug, Clone)]
pub struct VerifyReport {
    artifact_id: String,
    ok: bool,
    expected: Option<Fingerprint>,
    observed: Option<Fingerprint>,
    detail: String,
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
    artifact_id: String,
    script: Option<PathBuf>,
    receipt: PathBuf,
    uploaded: bool,
    published_file_id: Option<String>,
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

/// One valid, unregistered content project that may be explicitly adopted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProjectCandidate {
    /// Root directory of the unregistered content project.
    pub path: PathBuf,
    /// ID declared by the project's content manifest.
    pub id: String,
    /// Content kind declared by the project.
    pub kind: ContentKind,
}

/// Candidates found by a non-mutating project scan. Invalid folders are
/// returned as diagnostics and never enter the content catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceProjectScan {
    /// Valid unregistered projects available for explicit adoption.
    pub candidates: Vec<SourceProjectCandidate>,
    /// Reasons project folders were excluded from the candidate list.
    pub diagnostics: Vec<String>,
}

/// Find unregistered content folders for an explicit creator adoption action.
pub fn scan_unregistered_source_projects(
    paths: &EnvironmentPaths,
) -> Result<SourceProjectScan, String> {
    let workspace = WorkspaceManifest::load(paths)?;
    if workspace.kind() != crate::workspace::SourceRootKind::Workspace {
        return Err("content projects can only be discovered inside a Vapor Workspace".to_owned());
    }
    let source_root = fs::canonicalize(paths.source().root())
        .map_err(io("resolve source workspace", paths.source().root()))?;
    workspace_version(&source_root)?;
    let registered_ids = workspace
        .projects()
        .iter()
        .map(|project| project.id().to_owned())
        .collect::<BTreeSet<_>>();
    let registered_paths = workspace
        .projects()
        .iter()
        .filter_map(|project| fs::canonicalize(source_root.join(project.path())).ok())
        .collect::<BTreeSet<_>>();
    let mut scan = SourceProjectScan::default();
    let mut pending = vec![source_root.clone()];
    let mut candidates_by_id = BTreeMap::<String, SourceProjectCandidate>::new();
    let mut duplicate_ids = BTreeSet::new();
    let mut visited = 0usize;
    while let Some(directory) = pending.pop() {
        visited += 1;
        if visited > 100_000 {
            scan.diagnostics.push("Stopped scanning after 100,000 folders; narrow the Workspace or move generated files under an excluded folder.".to_owned());
            break;
        }
        let mut entries = Vec::new();
        match fs::read_dir(&directory) {
            Ok(directory_entries) => {
                for entry in directory_entries {
                    match entry {
                        Ok(entry) => entries.push(entry),
                        Err(error) => scan.diagnostics.push(format!(
                            "Could not inspect an entry in {}: {error}",
                            directory.display()
                        )),
                    }
                }
            }
            Err(error) => {
                scan.diagnostics.push(format!(
                    "Could not inspect {}: {error}",
                    directory.display()
                ));
                continue;
            }
        }
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries.into_iter().rev() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.')
                || matches!(
                    name.as_ref(),
                    "target" | "node_modules" | "vendor" | "output"
                )
            {
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    scan.diagnostics.push(format!(
                        "Could not inspect {}: {error}",
                        entry.path().display()
                    ));
                    continue;
                }
            };
            if !file_type.is_dir() {
                continue;
            }
            let path = match fs::canonicalize(entry.path()) {
                Ok(path) if path.starts_with(&source_root) => path,
                Ok(_) => continue,
                Err(error) => {
                    scan.diagnostics.push(format!(
                        "Could not resolve {}: {error}",
                        entry.path().display()
                    ));
                    continue;
                }
            };
            if registered_paths.contains(&path) {
                continue;
            }
            if let Some(marker) = manifest::content_marker_at(&path) {
                let marker_metadata = fs::symlink_metadata(&marker);
                let entity = match marker_metadata {
                    Ok(metadata)
                        if metadata.file_type().is_file() && !metadata.file_type().is_symlink() =>
                    {
                        manifest::read(&marker, &source_root)
                    }
                    Ok(_) => Err(format!(
                        "content manifest must be a regular non-symlink file: {}",
                        marker.display()
                    )),
                    Err(error) => Err(format!("could not inspect {}: {error}", marker.display())),
                };
                match entity {
                    Ok(VaporEntity::Content { id, kind, .. }) if !registered_ids.contains(&id) => {
                        match read_authored_content(&marker, kind) {
                            Ok(_) => match path.strip_prefix(&source_root).ok().and_then(Path::to_str) {
                                Some(relative) => {
                                    let candidate = SourceProjectCandidate {
                                        path: PathBuf::from(relative),
                                        id: id.clone(),
                                        kind,
                                    };
                                    if candidates_by_id.insert(id.clone(), candidate).is_some() {
                                        duplicate_ids.insert(id.clone());
                                    }
                                }
                                None => scan.diagnostics.push(format!("Project path is not valid Unicode and cannot be written to the Workspace manifest: {}", path.display())),
                            },
                            Err(error) => scan.diagnostics.push(error),
                        }
                    }
                    Ok(VaporEntity::Content { id, .. }) => scan.diagnostics.push(format!("{} duplicates registered content identity {id}", path.display())),
                    Ok(_) => scan.diagnostics.push(format!("{} is not a content project", marker.display())),
                    Err(error) => scan.diagnostics.push(error),
                }
                continue;
            }
            pending.push(path);
        }
    }
    for id in duplicate_ids {
        candidates_by_id.remove(&id);
        scan.diagnostics.push(format!("More than one unregistered folder declares content identity {id}; resolve the duplicate before adoption."));
    }
    scan.candidates = candidates_by_id.into_values().collect();
    scan.candidates
        .sort_by(|left, right| left.path.cmp(&right.path));
    scan.diagnostics.sort();
    Ok(scan)
}

/// Discover source-authored content artifacts in the active workspace.
///
/// # Errors
///
/// Returns filesystem, TOML, or manifest validation diagnostics.
pub fn discover(paths: &EnvironmentPaths) -> Result<ContentCatalog, String> {
    let source_root = paths.source().root();
    let workspace_version = workspace_version(source_root)?;
    let workspace = WorkspaceManifest::load(paths)?;

    let mut artifacts = Vec::new();
    for project in workspace.projects() {
        let kind = project.kind().content_kind();
        let manifest_path = source_root.join(project.manifest());
        match manifest::read(&manifest_path, source_root)? {
            VaporEntity::Content {
                kind: actual_kind,
                id,
                name,
            } => {
                if actual_kind != kind {
                    return Err(format!(
                        "registered workspace project '{}' changed kind from {kind} to {actual_kind}",
                        project.path().display()
                    ));
                }
                let root = manifest_path
                    .parent()
                    .ok_or_else(|| format!("manifest has no parent: {}", manifest_path.display()))?
                    .to_path_buf();
                let authored = read_authored_content(&manifest_path, kind)?;
                artifacts.push(ContentArtifact {
                    id,
                    name,
                    kind,
                    root,
                    manifest: manifest_path,
                    version: authored
                        .version
                        .resolve(workspace_version.as_ref())
                        .map(|version| version.to_string()),
                    binaries: authored.binaries,
                    libraries: authored.libraries,
                    runtime: Vec::new(),
                    dependencies: authored.dependencies,
                    conflicts: authored.conflicts,
                    workshop: authored.workshop,
                });
            }
            VaporEntity::Root { id, .. } | VaporEntity::Workspace { id, .. } => {
                return Err(format!(
                    "registered workspace project '{}' declares nested source root '{id}'",
                    project.path().display()
                ));
            }
            VaporEntity::Registry { id, .. } => {
                return Err(format!(
                    "registered workspace project '{}' declares registry '{id}'",
                    project.path().display()
                ));
            }
        }
    }
    artifacts.sort_by(|left, right| left.id.cmp(&right.id));
    let candidates = source_candidates(&artifacts)?;
    Ok(ContentCatalog {
        artifacts,
        candidates,
    })
}

fn source_candidates(artifacts: &[ContentArtifact]) -> Result<ContentCandidates, String> {
    let mut candidates = ContentCandidates::new();
    for artifact in artifacts {
        let Some(version) = artifact.version() else {
            continue;
        };
        let identity = ContentVersionId {
            id: artifact
                .id()
                .parse::<VaporId>()
                .map_err(|error| format!("invalid content ID '{}': {error}", artifact.id()))?,
            version: semver::Version::parse(version).map_err(|error| {
                format!("invalid content version '{version}' for '{}': {error}", artifact.id())
            })?,
        };
        let mut dependencies = BTreeMap::new();
        for (index, reference) in artifact.dependencies().iter().enumerate() {
            let id = reference
                .id()
                .parse::<VaporId>()
                .map_err(|error| format!("invalid dependency ID '{}': {error}", reference.id()))?;
            let requirement = reference
                .version()
                .unwrap_or("*")
                .parse::<semver::VersionReq>()
                .map_err(|error| {
                    format!(
                        "invalid dependency version requirement '{}' for '{}': {error}",
                        reference.version().unwrap_or("*"),
                        reference.id()
                    )
                })?;
            dependencies.insert(
                format!("{}#{index}", reference.relationship()),
                DependencySpec {
                    id,
                    version: requirement,
                    optional: reference.optional(),
                },
            );
        }
        candidates
            .insert(ContentCandidate {
                identity,
                kind: artifact.kind(),
                dependencies,
            })
            .map_err(|error| error.to_string())?;
    }
    Ok(candidates)
}

/// Create and register a role-specific content project in an authored workspace.
///
/// The generated project uses Vapor's manifest vocabulary and becomes
/// visible to the same catalog used by package, install, and Workshop flows.
pub fn create_source_project(
    paths: &EnvironmentPaths,
    content_id: &str,
    version: &str,
    kind: ContentKind,
    dependency_selectors: &[String],
) -> Result<PathBuf, String> {
    create_source_project_with_workshop(
        paths,
        content_id,
        version,
        kind,
        dependency_selectors,
        &WorkshopPolicyDraft::default(),
    )
}

/// Create a role-specific content project with optional Workshop publication metadata.
pub fn create_source_project_with_workshop(
    paths: &EnvironmentPaths,
    content_id: &str,
    version: &str,
    kind: ContentKind,
    dependency_selectors: &[String],
    workshop: &WorkshopPolicyDraft,
) -> Result<PathBuf, String> {
    create_source_project_with_dependency_policies(
        paths,
        content_id,
        version,
        kind,
        dependency_selectors,
        &[],
        workshop,
    )
}

/// Create a source project with selected optional dependency edges.
pub fn create_source_project_with_dependency_policies(
    paths: &EnvironmentPaths,
    content_id: &str,
    version: &str,
    kind: ContentKind,
    dependency_selectors: &[String],
    optional_dependency_selectors: &[String],
    workshop: &WorkshopPolicyDraft,
) -> Result<PathBuf, String> {
    validate_workshop_policy_draft(workshop)?;
    let workspace_manifest = paths.source().root().join(manifest::WORKSPACE_FILE_NAME);
    let _workspace_lock = acquire_workspace_project_lock(&workspace_manifest)?;
    let workspace = WorkspaceManifest::load(paths)?;
    if workspace.kind() != crate::workspace::SourceRootKind::Workspace {
        return Err("new content can only be added to a Vapor workspace".to_owned());
    }
    let prefix = format!("{}/", workspace.id());
    let name = content_id
        .strip_prefix(&prefix)
        .filter(|name| !name.is_empty() && !name.contains('/'))
        .ok_or_else(|| {
            format!("content ID must start with '{prefix}' and contain one local name")
        })?;
    if !name.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
    }) {
        return Err("content name must use lowercase ASCII letters, digits, '-' or '_'".to_owned());
    }
    let version = semver::Version::parse(version)
        .map_err(|error| format!("invalid content version: {error}"))?;
    if workspace
        .projects()
        .iter()
        .any(|project| project.id() == content_id)
    {
        return Err(format!(
            "content project is already registered: {content_id}"
        ));
    }

    let catalog = discover(paths)?;
    let mut children = Vec::new();
    let optional_selectors = optional_dependency_selectors
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut optional_identities = BTreeSet::new();
    for selector in optional_selectors {
        let child = catalog
            .find(selector)
            .ok_or_else(|| format!("optional content is no longer available: {selector}"))?;
        if source_dependency_relationship(kind, child.kind()) != Some("dependency") {
            return Err(format!(
                "{} is required for the '{}' role and cannot be optional",
                child.id(),
                source_dependency_relationship(kind, child.kind()).unwrap_or("dependency")
            ));
        }
        optional_identities.insert((
            child.id().to_owned(),
            child.version().map(ToOwned::to_owned),
        ));
    }
    for selector in dependency_selectors {
        let child = catalog
            .find(selector)
            .ok_or_else(|| format!("selected content is no longer available: {selector}"))?;
        if !source_dependency_allowed(kind, child.kind()) {
            return Err(format!("{} cannot be included in a {kind}", child.kind()));
        }
        if child.id() == content_id {
            return Err("content cannot depend on itself".to_owned());
        }
        children.push(child);
    }
    if optional_identities.iter().any(|(id, version)| {
        !children.iter().any(|child| {
            child.id() == id && child.version().map(ToOwned::to_owned).as_ref() == version.as_ref()
        })
    }) {
        return Err("optional content must also be selected for this project".to_owned());
    }
    validate_composition_children(kind, &children)?;

    let root = paths.source().root();
    let relative_root = PathBuf::from("content").join(name);
    let output = root.join(&relative_root);
    let original_workspace = fs::read_to_string(&workspace_manifest)
        .map_err(io("read workspace manifest", &workspace_manifest))?;
    let updated_workspace = format!(
        "{}\n\n[[workspace.projects]]\npath = \"content/{name}\"\n",
        original_workspace.trim_end(),
    );
    let cargo_transaction = if crate::source::content_project_output(kind).is_some() {
        let cargo_manifest = root.join("Cargo.toml");
        let original = fs::read_to_string(&cargo_manifest)
            .map_err(io("read Cargo workspace manifest", &cargo_manifest))?;
        let updated = cargo_workspace_with_member(&original, &relative_root)?;
        Some((cargo_manifest, original, updated))
    } else {
        None
    };
    let content_directory = root.join("content");
    let created_content_directory = match fs::symlink_metadata(&content_directory) {
        Ok(metadata) if metadata.file_type().is_dir() => false,
        Ok(_) => {
            return Err(format!(
                "{} is not a regular directory",
                content_directory.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&content_directory)
                .map_err(io("create workspace content directory", &content_directory))?;
            true
        }
        Err(error) => return Err(format!("{}: {error}", content_directory.display())),
    };
    let content_root = match fs::canonicalize(&content_directory) {
        Ok(path) => path,
        Err(error) => {
            if created_content_directory {
                let _ = fs::remove_dir(&content_directory);
            }
            return Err(format!(
                "failed to resolve workspace content directory '{}': {error}",
                content_directory.display()
            ));
        }
    };
    if !content_root.starts_with(root) {
        if created_content_directory {
            let _ = fs::remove_dir(&content_directory);
        }
        return Err(
            "workspace content directory resolves outside the selected workspace".to_owned(),
        );
    }
    if let Err(error) = fs::create_dir(&output) {
        if created_content_directory {
            let _ = fs::remove_dir(&content_directory);
        }
        return Err(io("create content project directory", &output)(error));
    }
    let manifest_path = output.join(kind.manifest_file_name());
    let create_result = (|| -> Result<(), String> {
        let template = crate::source::scaffold_content_project(
            &output,
            content_id,
            &version.to_string(),
            kind,
        )?;
        let body = source_project_manifest(
            name,
            &version.to_string(),
            kind,
            &children,
            &optional_identities,
            &template,
            workshop,
        );
        let mut manifest_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&manifest_path)
            .map_err(io("create content manifest", &manifest_path))?;
        manifest_file
            .write_all(body.as_bytes())
            .map_err(io("write content manifest", &manifest_path))?;
        if matches!(
            &template,
            crate::source::GeneratedContentProject::Rust { .. }
        ) {
            let readme_path = output.join("README.md");
            let mut readme = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&readme_path)
                .map_err(io("create content README", &readme_path))?;
            let role_manifest = manifest_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "content.vapor.toml".to_owned());
            writeln!(
                readme,
                "# {name}\n\nVapor {kind} content `{content_id}`.\n\n- `{role_manifest}` defines this content's identity and dependencies.\n- `Cargo.toml` registers the Rust project in the workspace.\n- `src/` contains generated starting code. Crate projects provide static Rust code; other code roles may declare runtime binaries or shared libraries in their role manifest.\n\nUse Vapor Studio to edit and validate this project, then package or deploy it through Vapor."
            )
            .map_err(io("write content README", &readme_path))?;
        }
        if let Some((cargo_manifest, original_cargo, _)) = &cargo_transaction {
            let current_cargo = fs::read_to_string(cargo_manifest)
                .map_err(io("re-read Cargo workspace manifest", cargo_manifest))?;
            if current_cargo != *original_cargo {
                return Err("Cargo workspace manifest changed while content was being prepared; retry the action".to_owned());
            }
        }
        let current_workspace = fs::read_to_string(&workspace_manifest)
            .map_err(io("re-read Workspace manifest", &workspace_manifest))?;
        if current_workspace != original_workspace {
            return Err(
                "Workspace manifest changed while content was being prepared; retry the action"
                    .to_owned(),
            );
        }
        if let Some((cargo_manifest, original_cargo, updated_cargo)) = &cargo_transaction {
            replace_source_manifest(
                cargo_manifest,
                original_cargo,
                updated_cargo,
                "Cargo workspace membership",
                || {
                    WorkspaceManifest::load(paths)?;
                    vapor_core::VaporWorkspace::load(root).map_err(|error| error.to_string())?;
                    Ok(())
                },
            )?;
        }
        replace_workspace_manifest_locked(
            &workspace_manifest,
            &original_workspace,
            &updated_workspace,
            "content project registration",
            || {
                WorkspaceManifest::load(paths)?;
                vapor_core::VaporWorkspace::load(root).map_err(|error| error.to_string())?;
                discover(paths)?;
                Ok(())
            },
        )?;
        Ok(())
    })();
    if let Err(error) = create_result {
        let mut recovery = Vec::new();
        match fs::read_to_string(&workspace_manifest) {
            Ok(current) if current == updated_workspace => {
                if let Err(restore_error) = replace_workspace_manifest_locked(
                    &workspace_manifest,
                    &updated_workspace,
                    &original_workspace,
                    "content project registration rollback",
                    || Ok(()),
                ) {
                    recovery.push(format!(
                        "could not restore {}: {restore_error}",
                        workspace_manifest.display()
                    ));
                }
            }
            Ok(current) if current == original_workspace => {}
            Ok(_) => recovery.push(format!(
                "{} changed during recovery and was left untouched",
                workspace_manifest.display()
            )),
            Err(error) => recovery.push(format!(
                "could not inspect {} during recovery: {error}",
                workspace_manifest.display()
            )),
        }
        if let Some((cargo_manifest, original_cargo, updated_cargo)) = &cargo_transaction {
            match fs::read_to_string(cargo_manifest) {
                Ok(current) if current == *updated_cargo => {
                    if let Err(restore_error) = replace_source_manifest(
                        cargo_manifest,
                        updated_cargo,
                        original_cargo,
                        "Cargo workspace membership rollback",
                        || Ok(()),
                    ) {
                        recovery.push(format!(
                            "could not restore {}: {restore_error}",
                            cargo_manifest.display()
                        ));
                    }
                }
                Ok(current) if current == *original_cargo => {}
                Ok(_) => recovery.push(format!(
                    "{} changed during recovery and was left untouched",
                    cargo_manifest.display()
                )),
                Err(error) => recovery.push(format!(
                    "could not inspect {} during recovery: {error}",
                    cargo_manifest.display()
                )),
            }
        }
        if let Err(cleanup_error) = fs::remove_dir_all(&output) {
            recovery.push(format!(
                "could not remove incomplete project {}: {cleanup_error}",
                output.display()
            ));
        }
        if created_content_directory
            && let Err(error) = fs::remove_dir(&content_directory)
            && error.kind() != std::io::ErrorKind::DirectoryNotEmpty
            && error.kind() != std::io::ErrorKind::NotFound
        {
            recovery.push(format!(
                "could not remove empty workspace content directory {}: {error}",
                content_directory.display()
            ));
        }
        if recovery.is_empty() {
            return Err(error);
        }
        return Err(format!(
            "{error}; recovery also reported: {}",
            recovery.join("; ")
        ));
    }
    Ok(manifest_path)
}

/// Register an existing in-workspace content project without moving its files.
pub fn adopt_source_project(
    paths: &EnvironmentPaths,
    project_path: &Path,
) -> Result<String, String> {
    let workspace = WorkspaceManifest::load(paths)?;
    if workspace.kind() != crate::workspace::SourceRootKind::Workspace {
        return Err("content projects can only be adopted into a Vapor Workspace".to_owned());
    }
    if project_path.as_os_str().is_empty()
        || project_path.is_absolute()
        || project_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(
            "project path must be a non-empty relative path inside the Workspace".to_owned(),
        );
    }

    let source_root = fs::canonicalize(paths.source().root())
        .map_err(io("resolve source workspace", paths.source().root()))?;
    let requested_root = source_root.join(project_path);
    let metadata = fs::symlink_metadata(&requested_root)
        .map_err(io("inspect project folder", &requested_root))?;
    if !metadata.file_type().is_dir() {
        return Err(format!(
            "project path must name a regular directory: {}",
            project_path.display()
        ));
    }
    let project_root =
        fs::canonicalize(&requested_root).map_err(io("resolve project folder", &requested_root))?;
    if !project_root.starts_with(&source_root) {
        return Err("project folder resolves outside the selected Workspace".to_owned());
    }
    let marker = manifest::content_marker_at(&project_root).ok_or_else(|| {
        format!(
            "no role-specific Vapor content manifest found in {}",
            project_path.display()
        )
    })?;
    let marker_metadata =
        fs::symlink_metadata(&marker).map_err(io("inspect content manifest", &marker))?;
    if !marker_metadata.file_type().is_file() || marker_metadata.file_type().is_symlink() {
        return Err(format!(
            "content manifest must be a regular non-symlink file: {}",
            marker.display()
        ));
    }
    let (id, kind) = match manifest::read(&marker, &source_root)? {
        VaporEntity::Content { id, kind, .. } => (id, kind),
        _ => {
            return Err(format!(
                "{} is not a content project manifest",
                marker.display()
            ));
        }
    };
    if workspace
        .projects()
        .iter()
        .any(|project| project.path() == project_path || project.id() == id)
    {
        return Err(format!("content project is already registered: {id}"));
    }

    let workspace_manifest = source_root.join(manifest::WORKSPACE_FILE_NAME);
    let workspace_metadata = fs::symlink_metadata(&workspace_manifest)
        .map_err(io("inspect Workspace manifest", &workspace_manifest))?;
    if !workspace_metadata.file_type().is_file() || workspace_metadata.file_type().is_symlink() {
        return Err(format!(
            "Workspace manifest must be a regular non-symlink file: {}",
            workspace_manifest.display()
        ));
    }
    let original = fs::read_to_string(&workspace_manifest)
        .map_err(io("read Workspace manifest", &workspace_manifest))?;
    let relative_project = project_root
        .strip_prefix(&source_root)
        .map_err(|error| format!("project folder is outside the Workspace: {error}"))?;
    let relative_project = relative_project
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    let mut document = original
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("Workspace manifest is not valid TOML: {error}"))?;
    let workspace_table = document
        .get_mut("workspace")
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| "Workspace manifest has no [workspace] table".to_owned())?;
    match workspace_table.get_mut("projects") {
        Some(toml_edit::Item::ArrayOfTables(projects)) => {
            let mut project = toml_edit::Table::new();
            project.insert("path", toml_edit::value(relative_project));
            projects.push(project);
        }
        Some(_) => return Err("workspace.projects must be an array of tables".to_owned()),
        None => {
            let mut projects = toml_edit::ArrayOfTables::new();
            let mut project = toml_edit::Table::new();
            project.insert("path", toml_edit::value(relative_project));
            projects.push(project);
            workspace_table.insert("projects", toml_edit::Item::ArrayOfTables(projects));
        }
    }

    let updated = document.to_string();
    replace_workspace_manifest(
        &workspace_manifest,
        &original,
        &updated,
        "content project registration",
        || {
            WorkspaceManifest::load(paths)?;
            vapor_core::VaporWorkspace::load(&source_root).map_err(|error| error.to_string())?;
            let catalog = discover(paths)?;
            if !catalog
                .artifacts()
                .iter()
                .any(|artifact| artifact.id() == id && artifact.kind() == kind)
            {
                return Err(format!(
                    "adopted content project {id} is not visible after registration"
                ));
            }
            Ok(())
        },
    )?;
    Ok(format!(
        "Registered {kind} content {id} from {}.",
        project_path.display()
    ))
}

/// Remove a content project from Vapor discovery without touching its files or Cargo membership.
pub fn unregister_source_project(paths: &EnvironmentPaths, id: &str) -> Result<String, String> {
    let workspace = WorkspaceManifest::load(paths)?;
    if workspace.kind() != crate::workspace::SourceRootKind::Workspace {
        return Err("content projects can only be unregistered from a Vapor Workspace".to_owned());
    }
    let project = workspace
        .projects()
        .iter()
        .find(|project| project.id() == id)
        .ok_or_else(|| format!("content project is not registered in this Workspace: {id}"))?;
    let project_path = project.path().to_path_buf();
    let project_kind = project.kind().to_string();
    let source_root = fs::canonicalize(paths.source().root())
        .map_err(io("resolve source workspace", paths.source().root()))?;
    let workspace_manifest = source_root.join(manifest::WORKSPACE_FILE_NAME);
    let metadata = fs::symlink_metadata(&workspace_manifest)
        .map_err(io("inspect Workspace manifest", &workspace_manifest))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "Workspace manifest must be a regular non-symlink file: {}",
            workspace_manifest.display()
        ));
    }
    let original = fs::read_to_string(&workspace_manifest)
        .map_err(io("read Workspace manifest", &workspace_manifest))?;
    let mut document = original
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("Workspace manifest is not valid TOML: {error}"))?;
    let workspace_table = document
        .get_mut("workspace")
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| "Workspace manifest has no [workspace] table".to_owned())?;
    let projects_item = workspace_table
        .get("projects")
        .ok_or_else(|| "Workspace has no registered content projects".to_owned())?;
    let projects = projects_item
        .as_array_of_tables()
        .ok_or_else(|| "workspace.projects must be an array of tables".to_owned())?;
    let mut retained = toml_edit::ArrayOfTables::new();
    let mut removed = 0usize;
    let mut retained_count = 0usize;
    for entry in projects.iter() {
        let matches = entry
            .get("path")
            .and_then(toml_edit::Item::as_str)
            .is_some_and(|path| Path::new(path) == project_path.as_path());
        if matches {
            removed += 1;
        } else {
            retained.push(entry.clone());
            retained_count += 1;
        }
    }
    if removed != 1 {
        return Err(format!(
            "expected one Workspace registration for {id}, found {removed}; no change made"
        ));
    }
    if retained_count == 0 {
        workspace_table.remove("projects");
    } else {
        workspace_table.insert("projects", toml_edit::Item::ArrayOfTables(retained));
    }
    let updated = document.to_string();
    replace_workspace_manifest(
        &workspace_manifest,
        &original,
        &updated,
        "content project unregistration",
        || {
            WorkspaceManifest::load(paths)?;
            vapor_core::VaporWorkspace::load(&source_root).map_err(|error| error.to_string())?;
            let catalog = discover(paths)?;
            if catalog
                .artifacts()
                .iter()
                .any(|artifact| artifact.id() == id)
            {
                return Err(format!(
                    "content project {id} remains visible after unregistration"
                ));
            }
            Ok(())
        },
    )?;
    Ok(format!(
        "Unregistered {project_kind} content {id} from the Vapor project registry. Its files and Cargo membership were left unchanged."
    ))
}

fn replace_workspace_manifest<T>(
    manifest_path: &Path,
    expected_source: &str,
    updated_source: &str,
    action: &str,
    validate: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _lock = acquire_workspace_project_lock(manifest_path)?;
    replace_workspace_manifest_locked(
        manifest_path,
        expected_source,
        updated_source,
        action,
        validate,
    )
}

fn replace_workspace_manifest_locked<T>(
    manifest_path: &Path,
    expected_source: &str,
    updated_source: &str,
    action: &str,
    validate: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let current = fs::read_to_string(manifest_path)
        .map_err(io("re-read Workspace manifest", manifest_path))?;
    if current != expected_source {
        return Err("Workspace manifest changed while the project registry edit was being prepared; retry the action".to_owned());
    }
    let temporary = source_manifest_temp_path(manifest_path);
    if let Err(error) = write_source_manifest_temp(&temporary, updated_source, manifest_path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let backup = source_manifest_temp_path_with_suffix(manifest_path, "backup");
    if let Err(error) = fs::rename(manifest_path, &backup) {
        let _ = fs::remove_file(&temporary);
        return Err(io("prepare Workspace manifest replacement", manifest_path)(
            error,
        ));
    }
    if let Err(error) = fs::rename(&temporary, manifest_path) {
        let restore = fs::rename(&backup, manifest_path);
        let _ = fs::remove_file(&temporary);
        return Err(match restore {
            Ok(()) => io("replace Workspace manifest", manifest_path)(error),
            Err(restore) => {
                format!("failed to apply {action}: {error}; recovery failed: {restore}")
            }
        });
    }
    match validate() {
        Ok(result) => {
            let _ = fs::remove_file(&backup);
            Ok(result)
        }
        Err(error) => {
            let _ = fs::remove_file(manifest_path);
            Err(match fs::rename(&backup, manifest_path) {
                Ok(()) => format!(
                    "{action} failed validation: {error}; original Workspace manifest restored"
                ),
                Err(restore) => {
                    format!("{action} failed validation: {error}; recovery failed: {restore}")
                }
            })
        }
    }
}

fn acquire_workspace_project_lock(manifest_path: &Path) -> Result<SourceManifestEditLock, String> {
    let lock_path = manifest_path.with_file_name(".Workspace.vapor-project.lock");
    let lock_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|error| {
            format!(
                "cannot acquire Workspace edit lock '{}': {error}",
                lock_path.display()
            )
        })?;
    Ok(SourceManifestEditLock {
        path: lock_path,
        file: Some(lock_file),
    })
}

/// Load the editable dependency graph for a locally authored content project.
pub fn load_source_composition(
    paths: &EnvironmentPaths,
    selector: &str,
) -> Result<SourceComposition, String> {
    let (artifact, manifest, source) = load_source_manifest_snapshot(paths, selector)?;
    Ok(SourceComposition {
        id: artifact.id().to_owned(),
        version: artifact.version().map(ToOwned::to_owned),
        kind: artifact.kind().to_string(),
        manifest,
        source,
        links: artifact
            .dependencies()
            .iter()
            .map(CompositionLink::from)
            .collect(),
    })
}

/// Replace a source content project's dependency graph after validating the exact
/// source snapshot and each selected edge against the live workspace catalog.
pub fn save_source_composition(
    paths: &EnvironmentPaths,
    selector: &str,
    expected_source: &str,
    links: &[CompositionLink],
) -> Result<SourceComposition, String> {
    let current = load_source_composition(paths, selector)?;
    if current.source != expected_source {
        return Err(
            "content manifest changed on disk since it was opened; reload it before saving"
                .to_owned(),
        );
    }
    let catalog = discover(paths)?;
    let root = catalog.find(selector).ok_or_else(|| {
        format!("source content is no longer available in this workspace: {selector}")
    })?;
    let root_id = root.id().to_owned();
    let root_kind = root.kind();
    let mut selected = Vec::with_capacity(links.len());
    let mut seen = BTreeSet::new();
    for link in links {
        let selector_key = format!("{}\0{}", link.relationship, link.id);
        if !seen.insert(selector_key) {
            return Err(format!(
                "duplicate composition edge: {} ({})",
                link.id, link.relationship
            ));
        }
        let requirement = link
            .version
            .as_deref()
            .map(semver::VersionReq::parse)
            .transpose()
            .map_err(|error| format!("invalid version requirement for {}: {error}", link.id))?;
        let child = find_local_source(&catalog, &link.id, requirement.as_ref())
            .ok_or_else(|| format!("selected content is no longer available at the requested version in this workspace: {} {}", link.id, link.version.as_deref().unwrap_or("")))?;
        if child.id() == root_id {
            return Err("source content cannot depend on itself".to_owned());
        }
        let expected_relationship = source_dependency_relationship(root_kind, child.kind())
            .ok_or_else(|| format!("{} cannot be included in a {root_kind}", child.kind()))?;
        validate_dependency_edge(root_kind, &link.relationship, child)?;
        let preserves_existing_relationship = expected_relationship == "dependency"
            && current.links.iter().any(|existing| {
                existing.id == link.id && existing.relationship == link.relationship
            });
        if link.relationship != expected_relationship && !preserves_existing_relationship {
            return Err(format!(
                "{} must use the '{}' relationship, not '{}'",
                child.id(),
                expected_relationship,
                link.relationship
            ));
        }
        if link.optional && matches!(link.relationship.as_str(), "engine" | "enginepack") {
            return Err(format!(
                "{} cannot mark its required Engine relationship optional",
                root_kind
            ));
        }
        if link.optional && root_kind == ContentKind::Gamepack && link.relationship == "game" {
            return Err(
                "a Gamepack cannot mark its required Game relationship optional".to_owned(),
            );
        }
        selected.push(child);
    }
    validate_composition_children(root_kind, &selected)?;
    validate_local_source_graph(&catalog, &root_id, &selected)?;

    let mut document = expected_source
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("content manifest is not valid TOML: {error}"))?;
    let role = root_kind.to_string();
    let role_table = document
        .get_mut(role.as_str())
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| format!("source manifest has no [{role}] table"))?;
    let mut existing_direct_edges = BTreeMap::new();
    for relationship in ["engine", "enginepack", "game", "gamepack"] {
        if let Some(toml_edit::Item::Table(table)) = role_table.remove(relationship) {
            existing_direct_edges.insert(relationship.to_owned(), table);
        }
    }
    let mut existing_dependencies = BTreeMap::new();
    if let Some(toml_edit::Item::ArrayOfTables(dependencies)) = role_table.remove("dependencies") {
        for table in dependencies.iter() {
            let Some(id) = table.get("id").and_then(toml_edit::Item::as_str) else {
                continue;
            };
            let relationship = table
                .get("relationship")
                .and_then(toml_edit::Item::as_str)
                .unwrap_or("dependency");
            existing_dependencies.insert((id.to_owned(), relationship.to_owned()), table.clone());
        }
    }
    let mut dependencies = toml_edit::ArrayOfTables::new();
    for (link, child) in links.iter().zip(&selected) {
        let expected_relationship = source_dependency_relationship(root_kind, child.kind())
            .ok_or_else(|| format!("{} cannot be included in a {root_kind}", child.kind()))?;
        if expected_relationship != "dependency" {
            let mut reference = existing_direct_edges
                .remove(&link.relationship)
                .unwrap_or_else(toml_edit::Table::new);
            update_composition_link_fields(&mut reference, link, false);
            role_table.insert(
                link.relationship.as_str(),
                toml_edit::Item::Table(reference),
            );
        } else {
            let mut reference = existing_dependencies
                .remove(&(link.id.clone(), link.relationship.clone()))
                .unwrap_or_else(toml_edit::Table::new);
            update_composition_link_fields(&mut reference, link, true);
            dependencies.push(reference);
        }
    }
    if !dependencies.is_empty() {
        role_table.insert("dependencies", toml_edit::Item::ArrayOfTables(dependencies));
    }
    let updated = document.to_string();
    replace_source_manifest(
        &current.manifest,
        expected_source,
        &updated,
        "content graph",
        || load_source_composition(paths, selector),
    )
}

/// Load the authored Workshop policy and exact source snapshot for a content project.
pub fn load_source_workshop_metadata(
    paths: &EnvironmentPaths,
    selector: &str,
) -> Result<SourceWorkshopMetadata, String> {
    let (artifact, manifest, source) = load_source_manifest_snapshot(paths, selector)?;
    let policy = artifact.workshop();
    Ok(SourceWorkshopMetadata {
        id: artifact.id().to_owned(),
        version: artifact.version().map(ToOwned::to_owned),
        kind: artifact.kind().to_string(),
        manifest,
        source,
        policy: WorkshopPolicyDraft {
            app_id: policy.app_id(),
            published_file_id: policy.published_file_id().map(ToOwned::to_owned),
            visibility: policy.visibility().map(ToOwned::to_owned),
            title: policy.title().map(ToOwned::to_owned),
            description: policy.description().map(ToOwned::to_owned),
            tags: policy.tags().to_vec(),
            change_note: policy.change_note().map(ToOwned::to_owned),
        },
    })
}

/// Load the typed runtime output declarations and exact source snapshot.
pub fn load_source_runtime_outputs(
    paths: &EnvironmentPaths,
    selector: &str,
) -> Result<SourceRuntimeOutputs, String> {
    let (artifact, manifest, source) = load_source_manifest_snapshot(paths, selector)?;
    Ok(SourceRuntimeOutputs {
        id: artifact.id().to_owned(),
        version: artifact.version().map(ToOwned::to_owned),
        kind: artifact.kind().to_string(),
        manifest,
        source,
        binaries: artifact.binaries().to_vec(),
        libraries: artifact.libraries().to_vec(),
    })
}

/// Save runtime output declarations through the shared guarded manifest writer.
pub fn save_source_runtime_outputs(
    paths: &EnvironmentPaths,
    selector: &str,
    expected_source: &str,
    binaries: &[String],
    libraries: &[String],
) -> Result<SourceRuntimeOutputs, String> {
    let current = load_source_runtime_outputs(paths, selector)?;
    if current.source != expected_source {
        return Err(
            "content manifest changed on disk since it was opened; reload before saving".to_owned(),
        );
    }
    validate_runtime_output_names("binaries", binaries)?;
    validate_runtime_output_names("libraries", libraries)?;
    let mut document = expected_source
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("content manifest is not valid TOML: {error}"))?;
    let role = &current.kind;
    let role_table = document
        .get_mut(role.as_str())
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| format!("content manifest has no [{role}] table"))?;
    set_string_array(role_table, "binaries", binaries);
    set_string_array(role_table, "libraries", libraries);
    let updated = document.to_string();
    replace_source_manifest(
        &current.manifest,
        expected_source,
        &updated,
        "runtime output declarations",
        || load_source_runtime_outputs(paths, selector),
    )
}

fn load_source_manifest_snapshot(
    paths: &EnvironmentPaths,
    selector: &str,
) -> Result<(ContentArtifact, PathBuf, String), String> {
    let catalog = discover(paths)?;
    let artifact = catalog.find(selector).cloned().ok_or_else(|| {
        format!("source content is no longer available in this workspace: {selector}")
    })?;
    let root = fs::canonicalize(paths.source().root())
        .map_err(io("resolve source workspace", paths.source().root()))?;
    let metadata = fs::symlink_metadata(artifact.manifest())
        .map_err(io("inspect content manifest", artifact.manifest()))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(format!(
            "content manifest must be a regular file: {}",
            artifact.manifest().display()
        ));
    }
    let manifest = fs::canonicalize(artifact.manifest())
        .map_err(io("resolve content manifest", artifact.manifest()))?;
    if !manifest.starts_with(&root) {
        return Err("content manifest resolves outside the selected workspace".to_owned());
    }
    let source = fs::read_to_string(&manifest).map_err(io("read content manifest", &manifest))?;
    Ok((artifact, manifest, source))
}

fn replace_source_manifest<T>(
    manifest: &Path,
    expected_source: &str,
    updated_source: &str,
    subject: &str,
    reopen: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let lock_path = manifest.with_extension("vapor-edit.lock");
    let lock_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|error| {
            format!(
                "cannot acquire content manifest edit lock '{}': {error}",
                lock_path.display()
            )
        })?;
    let _lock = SourceManifestEditLock {
        path: lock_path,
        file: Some(lock_file),
    };
    let locked_source =
        fs::read_to_string(manifest).map_err(io("re-read content manifest", manifest))?;
    if locked_source != expected_source {
        return Err(
            "content manifest changed while the edit was being prepared; reload before saving"
                .to_owned(),
        );
    }
    let temporary = source_manifest_temp_path(manifest);
    if let Err(error) = write_source_manifest_temp(&temporary, updated_source, manifest) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    let backup = source_manifest_temp_path_with_suffix(manifest, "backup");
    if let Err(error) = fs::rename(manifest, &backup) {
        let _ = fs::remove_file(&temporary);
        return Err(io("prepare content manifest replacement", manifest)(error));
    }
    if let Err(error) = fs::rename(&temporary, manifest) {
        let restore = fs::rename(&backup, manifest);
        let _ = fs::remove_file(&temporary);
        return Err(match restore {
            Ok(()) => io("replace content manifest", manifest)(error),
            Err(restore) => {
                format!("failed to replace content manifest: {error}; recovery failed: {restore}")
            }
        });
    }
    let reopened = reopen()
        .map_err(|error| format!("updated {subject} could not be discovered or reopened: {error}"));
    match reopened {
        Ok(value) => {
            let _ = fs::remove_file(&backup);
            Ok(value)
        }
        Err(error) => {
            let _ = fs::remove_file(manifest);
            Err(match fs::rename(&backup, manifest) {
                Ok(()) => format!(
                    "updated {subject} could not be reopened: {error}; original manifest restored"
                ),
                Err(restore) => format!(
                    "updated {subject} could not be reopened: {error}; recovery failed: {restore}"
                ),
            })
        }
    }
}

fn set_string_array(table: &mut toml_edit::Table, key: &str, values: &[String]) {
    if values.is_empty() {
        table.remove(key);
        return;
    }
    let mut array = toml_edit::Array::new();
    for value in values {
        array.push(value.as_str());
    }
    table.insert(key, toml_edit::Item::Value(toml_edit::Value::Array(array)));
}

/// Save Workshop policy through a stale-snapshot-checked, atomic manifest replacement.
pub fn save_source_workshop_metadata(
    paths: &EnvironmentPaths,
    selector: &str,
    expected_source: &str,
    policy: &WorkshopPolicyDraft,
) -> Result<SourceWorkshopMetadata, String> {
    let current = load_source_workshop_metadata(paths, selector)?;
    if current.source != expected_source {
        return Err(
            "content manifest changed on disk since it was opened; reload before saving".to_owned(),
        );
    }
    validate_workshop_policy_draft(policy)?;
    let mut document = expected_source
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("content manifest is not valid TOML: {error}"))?;
    let role = &current.kind;
    let role_table = document
        .get_mut(role.as_str())
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| format!("content manifest has no [{role}] table"))?;
    if role_table.get("steam").is_none() {
        role_table.insert("steam", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let steam_item = role_table
        .get_mut("steam")
        .ok_or_else(|| format!("[{role}.steam] could not be created"))?;
    let steam = steam_item
        .as_table_mut()
        .ok_or_else(|| format!("[{role}.steam] must be a table"))?;
    set_optional_toml_value(
        steam,
        "app-id",
        policy
            .app_id
            .map(|app_id| toml_edit::value(i64::from(app_id))),
    );
    set_optional_toml_value(
        steam,
        "published-file-id",
        policy.published_file_id.as_deref().map(toml_edit::value),
    );
    set_optional_toml_value(
        steam,
        "visibility",
        policy.visibility.as_deref().map(toml_edit::value),
    );
    set_optional_toml_value(
        steam,
        "title",
        policy.title.as_deref().map(toml_edit::value),
    );
    set_optional_toml_value(
        steam,
        "description",
        policy.description.as_deref().map(toml_edit::value),
    );
    let tags = if policy.tags.is_empty() {
        None
    } else {
        let mut tags = toml_edit::Array::new();
        for tag in &policy.tags {
            tags.push(tag.as_str());
        }
        Some(toml_edit::Item::Value(toml_edit::Value::Array(tags)))
    };
    set_optional_toml_value(steam, "tags", tags);
    set_optional_toml_value(
        steam,
        "change-note",
        policy.change_note.as_deref().map(toml_edit::value),
    );
    let remove_steam_table = steam.is_empty();
    if remove_steam_table {
        role_table.remove("steam");
    }
    replace_source_manifest(
        &current.manifest,
        expected_source,
        &document.to_string(),
        "Workshop metadata",
        || load_source_workshop_metadata(paths, selector),
    )
}

fn set_optional_toml_value(
    table: &mut toml_edit::Table,
    key: &str,
    value: Option<toml_edit::Item>,
) {
    if let Some(value) = value {
        table.insert(key, value);
    } else {
        table.remove(key);
    }
}

fn update_composition_link_fields(
    table: &mut toml_edit::Table,
    link: &CompositionLink,
    write_relationship: bool,
) {
    table.insert("id", toml_edit::value(link.id.clone()));
    set_optional_toml_value(
        table,
        "version",
        link.version.as_deref().map(toml_edit::value),
    );
    set_optional_toml_value(
        table,
        "optional",
        link.optional.then(|| toml_edit::value(true)),
    );
    set_optional_toml_value(
        table,
        "workshop_id",
        link.workshop_id.as_deref().map(toml_edit::value),
    );
    set_optional_toml_value(
        table,
        "reason",
        link.reason.as_deref().map(toml_edit::value),
    );
    if write_relationship {
        set_optional_toml_value(
            table,
            "relationship",
            (link.relationship != "dependency")
                .then(|| toml_edit::value(link.relationship.clone())),
        );
    }
}

fn validate_workshop_policy_draft(policy: &WorkshopPolicyDraft) -> Result<(), String> {
    if policy.app_id.is_some_and(|app_id| app_id == 0) {
        return Err("Steam AppID must be positive".to_owned());
    }
    if policy
        .published_file_id
        .as_deref()
        .is_some_and(|id| id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err("PublishedFileId must contain only decimal digits".to_owned());
    }
    Ok(())
}

fn find_local_source<'a>(
    catalog: &'a ContentCatalog,
    id: &str,
    requirement: Option<&semver::VersionReq>,
) -> Option<&'a ContentArtifact> {
    catalog.artifacts().iter().find(|artifact| {
        artifact.id() == id
            && requirement.is_none_or(|requirement| {
                artifact
                    .version()
                    .and_then(|version| semver::Version::parse(version).ok())
                    .is_some_and(|version| requirement.matches(&version))
            })
    })
}

fn validate_local_source_graph(
    catalog: &ContentCatalog,
    root_id: &str,
    roots: &[&ContentArtifact],
) -> Result<(), String> {
    fn visit(
        catalog: &ContentCatalog,
        root_id: &str,
        artifact: &ContentArtifact,
        visiting: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) -> Result<(), String> {
        let id = artifact.id();
        if id == root_id {
            return Err(format!(
                "composition graph would cycle back to root content {root_id}"
            ));
        }
        if visited.contains(id) {
            return Ok(());
        }
        if !visiting.insert(id.to_owned()) {
            return Err(format!("source content dependency cycle reaches {id}"));
        }
        for dependency in artifact.dependencies() {
            let requirement = dependency
                .version()
                .map(semver::VersionReq::parse)
                .transpose()
                .map_err(|error| {
                    format!(
                        "{} has an invalid version requirement for {}: {error}",
                        artifact.id(),
                        dependency.id()
                    )
                })?;
            let child = find_local_source(catalog, dependency.id(), requirement.as_ref());
            let Some(child) = child else {
                if dependency.optional() {
                    // An absent optional node is not in this resolved local graph.
                    continue;
                }
                return Err(format!(
                    "local composition rule: {} requires {} {}, but no matching version is authored in this workspace",
                    artifact.id(),
                    dependency.id(),
                    dependency.version().unwrap_or_default()
                ));
            };
            // Traverse optional nodes too when they are locally available: their
            // own required descendants and cycles are still part of this graph.
            visit(catalog, root_id, child, visiting, visited)?;
        }
        visiting.remove(id);
        visited.insert(id.to_owned());
        Ok(())
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for root in roots {
        visit(catalog, root_id, root, &mut visiting, &mut visited)?;
    }
    Ok(())
}

struct SourceManifestEditLock {
    path: PathBuf,
    file: Option<fs::File>,
}
impl Drop for SourceManifestEditLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

fn source_manifest_temp_path(manifest: &Path) -> PathBuf {
    source_manifest_temp_path_with_suffix(manifest, "tmp")
}
fn source_manifest_temp_path_with_suffix(manifest: &Path, suffix: &str) -> PathBuf {
    let name = manifest
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("manifest"));
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    manifest.with_file_name(format!(
        ".{}.{}.{nonce}.{suffix}",
        name.to_string_lossy(),
        std::process::id()
    ))
}
fn write_source_manifest_temp(path: &Path, contents: &str, original: &Path) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io("create temporary source manifest", path))?;
    file.write_all(contents.as_bytes())
        .map_err(io("write temporary source manifest", path))?;
    file.sync_all()
        .map_err(io("sync temporary source manifest", path))?;
    if let Ok(metadata) = fs::metadata(original) {
        fs::set_permissions(path, metadata.permissions())
            .map_err(io("preserve source manifest permissions", path))?;
    }
    Ok(())
}

/// Whether a source content role can directly depend on another role.
///
/// Packs expose their documented member kinds, while non-pack content may
/// compose other capabilities through ordinary dependencies.
pub fn source_dependency_allowed(parent: ContentKind, child: ContentKind) -> bool {
    vapor_content::dependency_kind_allowed(parent, child)
}

fn validate_artifact_dependency_roles(artifact: &ContentArtifact) -> Result<(), String> {
    let relationships = artifact
        .dependencies()
        .iter()
        .map(|dependency| dependency.relationship().to_owned())
        .collect::<Vec<_>>();
    validate_source_relationship_cardinality(artifact.kind(), &relationships)
        .map_err(|error| format!("{}: {error}", artifact.id()))?;
    for dependency in artifact
        .dependencies()
        .iter()
        .filter(|edge| edge.optional())
    {
        if matches!(
            (artifact.kind(), dependency.relationship()),
            (
                ContentKind::Packagepack,
                "engine" | "enginepack" | "game" | "gamepack"
            ) | (ContentKind::Game, "engine")
                | (ContentKind::Enginepack, "engine")
                | (ContentKind::Gamepack, "game")
        ) {
            return Err(format!(
                "{} cannot mark its required '{}' relationship optional",
                artifact.id(),
                dependency.relationship()
            ));
        }
    }
    Ok(())
}

fn validate_dependency_edge(
    parent: ContentKind,
    relationship: &str,
    child: &ContentArtifact,
) -> Result<(), String> {
    if !source_dependency_allowed(parent, child.kind()) {
        return Err(format!(
            "{parent} cannot depend on {} content {}",
            child.kind(),
            child.id()
        ));
    }
    if let Some(expected) = source_dependency_relationship(parent, child.kind())
        && expected != "dependency"
        && relationship != expected
    {
        return Err(format!(
            "{} dependency '{}' must use the '{}' relationship, not '{}'",
            parent,
            child.id(),
            expected,
            relationship
        ));
    }
    if source_dependency_relationship(parent, child.kind()) == Some("dependency")
        && matches!(relationship, "engine" | "enginepack" | "game" | "gamepack")
    {
        return Err(format!(
            "{} content {} cannot use the '{}' composition role; use 'dependency'",
            child.kind(),
            child.id(),
            relationship
        ));
    }
    Ok(())
}

/// Return the authored edge name for a supported parent/child role pair.
pub fn source_dependency_relationship(
    parent: ContentKind,
    child: ContentKind,
) -> Option<&'static str> {
    if !source_dependency_allowed(parent, child) {
        return None;
    }
    Some(match (parent, child) {
        (ContentKind::Packagepack, ContentKind::Engine) => "engine",
        (ContentKind::Packagepack, ContentKind::Enginepack) => "enginepack",
        (ContentKind::Packagepack, ContentKind::Game) => "game",
        (ContentKind::Packagepack, ContentKind::Gamepack) => "gamepack",
        (ContentKind::Game, ContentKind::Engine) => "engine",
        (ContentKind::Enginepack, ContentKind::Engine) => "engine",
        (ContentKind::Gamepack, ContentKind::Game) => "game",
        _ => "dependency",
    })
}

/// Return the exclusive authored role slot for dependency choices with bounded
/// cardinality. Dependencies outside these role slots remain freely composable.
pub fn source_dependency_choice_group(
    parent: ContentKind,
    child: ContentKind,
) -> Option<&'static str> {
    source_relationship_choice_group(parent, source_dependency_relationship(parent, child)?)
}

fn source_relationship_choice_group(
    parent: ContentKind,
    relationship: &str,
) -> Option<&'static str> {
    match (parent, relationship) {
        (ContentKind::Packagepack, "engine" | "enginepack") => Some("engine-choice"),
        (ContentKind::Packagepack, "game" | "gamepack") => Some("game-choice"),
        (ContentKind::Game | ContentKind::Enginepack, "engine") => Some("engine"),
        (ContentKind::Gamepack, "game") => Some("game"),
        _ => None,
    }
}

/// Return every exclusive role slot that must be populated before this content
/// kind can form a valid project.
pub fn source_required_dependency_choice_groups(parent: ContentKind) -> &'static [&'static str] {
    match parent {
        ContentKind::Packagepack => &["engine-choice", "game-choice"],
        ContentKind::Game | ContentKind::Enginepack => &["engine"],
        ContentKind::Gamepack => &["game"],
        _ => &[],
    }
}

/// Create the maintained Engine/Game/Packagepack starter workspace at a new
/// or empty destination, using the installed app's Steam AppID.
pub fn create_basic_content_workspace(
    installation: &InstallationPaths,
    path: PathBuf,
    organization: &str,
    name: &str,
) -> Result<PathBuf, String> {
    let app_id = crate::source::installation_app_id(installation)?;
    let report = crate::source::init_basic_content(crate::source::BasicContentInit {
        path,
        organization: organization.to_owned(),
        name: name.to_owned(),
        app_id,
    })?;
    Ok(report.root().to_path_buf())
}

fn validate_composition_children(
    kind: ContentKind,
    children: &[&ContentArtifact],
) -> Result<(), String> {
    let relationships = children
        .iter()
        .filter_map(|child| source_dependency_relationship(kind, child.kind()))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    validate_source_relationship_cardinality(kind, &relationships)
}

fn validate_source_relationship_cardinality(
    kind: ContentKind,
    relationships: &[String],
) -> Result<(), String> {
    let count_group = |group: &str| {
        relationships
            .iter()
            .filter(|relationship| {
                source_relationship_choice_group(kind, relationship.as_str()) == Some(group)
            })
            .count()
    };
    for required_group in source_required_dependency_choice_groups(kind) {
        if count_group(required_group) != 1 {
            let message = match *required_group {
                "engine-choice" => {
                    "a Packagepack needs exactly one Engine or Enginepack dependency"
                }
                "game-choice" => "a Packagepack needs exactly one Game or Gamepack dependency",
                "engine" if kind == ContentKind::Game => {
                    "a Game needs exactly one Engine dependency"
                }
                "engine" => "an Enginepack needs exactly one Engine",
                "game" => "a Gamepack needs exactly one Game",
                _ => "content is missing a required role dependency",
            };
            return Err(message.to_owned());
        }
    }
    Ok(())
}

fn cargo_workspace_with_member(source: &str, member: &Path) -> Result<String, String> {
    let member = member.to_string_lossy().replace('\\', "/");
    let mut document = source
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| format!("Cargo.toml is not valid TOML: {error}"))?;
    if document.get("workspace").is_none() {
        document.insert("workspace", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let workspace = document
        .get_mut("workspace")
        .and_then(toml_edit::Item::as_table_mut)
        .ok_or_else(|| {
            "Cargo.toml [workspace] must be a table before content can be added".to_owned()
        })?;

    let excluded = workspace
        .get("exclude")
        .and_then(toml_edit::Item::as_array)
        .is_some_and(|patterns| {
            patterns
                .iter()
                .filter_map(toml_edit::Value::as_str)
                .any(|pattern| cargo_member_pattern_matches(pattern, &member))
        });
    if excluded {
        return Err(format!(
            "Cargo.toml excludes '{member}' from the workspace; remove that exclusion before creating the content project"
        ));
    }
    if workspace.get("members").is_none() {
        workspace.insert(
            "members",
            toml_edit::Item::Value(toml_edit::Value::Array(toml_edit::Array::new())),
        );
    }
    let members = workspace
        .get_mut("members")
        .and_then(toml_edit::Item::as_array_mut)
        .ok_or_else(|| "Cargo.toml [workspace].members must be an array".to_owned())?;
    let covered = members
        .iter()
        .filter_map(toml_edit::Value::as_str)
        .any(|pattern| cargo_member_pattern_matches(pattern, &member));
    if !covered {
        members.push(member);
    }
    Ok(document.to_string())
}

fn cargo_member_pattern_matches(pattern: &str, member: &str) -> bool {
    fn segment_matches(pattern: &str, value: &str) -> bool {
        let pattern = pattern.chars().collect::<Vec<_>>();
        let value = value.chars().collect::<Vec<_>>();
        let (mut pattern_index, mut value_index) = (0, 0);
        let mut last_star = None;
        let mut star_value_index = 0;
        while value_index < value.len() {
            if pattern_index < pattern.len()
                && (pattern[pattern_index] == '?' || pattern[pattern_index] == value[value_index])
            {
                pattern_index += 1;
                value_index += 1;
            } else if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
                last_star = Some(pattern_index);
                pattern_index += 1;
                star_value_index = value_index;
            } else if let Some(star) = last_star {
                pattern_index = star + 1;
                star_value_index += 1;
                value_index = star_value_index;
            } else {
                return false;
            }
        }
        while pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            pattern_index += 1;
        }
        pattern_index == pattern.len()
    }
    fn match_segments(
        patterns: &[&str],
        values: &[&str],
        pattern_index: usize,
        value_index: usize,
    ) -> bool {
        if pattern_index == patterns.len() {
            return value_index == values.len();
        }
        if patterns[pattern_index] == "**" {
            return (value_index..=values.len())
                .any(|next| match_segments(patterns, values, pattern_index + 1, next));
        }
        value_index < values.len()
            && segment_matches(patterns[pattern_index], values[value_index])
            && match_segments(patterns, values, pattern_index + 1, value_index + 1)
    }
    let patterns = pattern.split('/').collect::<Vec<_>>();
    let values = member.split('/').collect::<Vec<_>>();
    match_segments(&patterns, &values, 0, 0)
}

fn source_project_manifest(
    name: &str,
    version: &str,
    kind: ContentKind,
    children: &[&ContentArtifact],
    optional_identities: &BTreeSet<(String, Option<String>)>,
    template: &crate::source::GeneratedContentProject,
    workshop: &WorkshopPolicyDraft,
) -> String {
    let mut output =
        format!("schema = 1\n\n[{kind}]\nname = \"{name}\"\nversion = \"{version}\"\n");
    match template {
        crate::source::GeneratedContentProject::Rust {
            crate_name,
            output: output_kind,
        } => match output_kind {
            crate::source::GeneratedOutputKind::Binary => {
                output.push_str(&format!("binaries = [\"{crate_name}\"]\n"));
            }
            crate::source::GeneratedOutputKind::RuntimeLibrary => {
                output.push_str(&format!("libraries = [\"{crate_name}\"]\n"));
            }
            crate::source::GeneratedOutputKind::StaticCrate => {}
        },
        crate::source::GeneratedContentProject::MetadataOnly => {}
    }
    let mut used = BTreeSet::new();
    for child in children {
        let edge = match (kind, child.kind()) {
            (ContentKind::Packagepack, ContentKind::Engine) => Some("engine"),
            (ContentKind::Packagepack, ContentKind::Enginepack) => Some("enginepack"),
            (ContentKind::Packagepack, ContentKind::Game) => Some("game"),
            (ContentKind::Packagepack, ContentKind::Gamepack) => Some("gamepack"),
            (ContentKind::Game, ContentKind::Engine) => Some("engine"),
            (ContentKind::Enginepack, ContentKind::Engine) => Some("engine"),
            (ContentKind::Gamepack, ContentKind::Game) => Some("game"),
            _ => None,
        };
        if let Some(edge) = edge {
            if used.insert(edge) {
                output.push_str(&format!("\n[{kind}.{edge}]\nid = \"{}\"\n", child.id()));
                if let Some(version) = child.version() {
                    output.push_str(&format!("version = \"={version}\"\n"));
                }
            }
        } else {
            output.push_str(&format!(
                "\n[[{kind}.dependencies]]\nid = \"{}\"\n",
                child.id()
            ));
            if let Some(version) = child.version() {
                output.push_str(&format!("version = \"={version}\"\n"));
            }
            if optional_identities.contains(&(
                child.id().to_owned(),
                child.version().map(ToOwned::to_owned),
            )) {
                output.push_str("optional = true\n");
            }
        }
    }
    if !workshop.is_empty() {
        output.push_str(&format!("\n[{kind}.steam]\n"));
        if let Some(app_id) = workshop.app_id {
            output.push_str(&format!("app-id = {app_id}\n"));
        }
        if let Some(published_file_id) = &workshop.published_file_id {
            output.push_str(&format!(
                "published-file-id = {}\n",
                toml_edit::value(published_file_id)
            ));
        }
        if let Some(visibility) = &workshop.visibility {
            output.push_str(&format!("visibility = {}\n", toml_edit::value(visibility)));
        }
        if let Some(title) = &workshop.title {
            output.push_str(&format!("title = {}\n", toml_edit::value(title)));
        }
        if let Some(description) = &workshop.description {
            output.push_str(&format!(
                "description = {}\n",
                toml_edit::value(description)
            ));
        }
        if !workshop.tags.is_empty() {
            let mut tags = toml_edit::Array::new();
            for tag in &workshop.tags {
                tags.push(tag.as_str());
            }
            output.push_str(&format!("tags = {tags}\n"));
        }
        if let Some(change_note) = &workshop.change_note {
            output.push_str(&format!(
                "change-note = {}\n",
                toml_edit::value(change_note)
            ));
        }
    }
    output
}

/// Validate content dependency, conflict, and Workshop metadata.
///
/// # Errors
///
/// Returns an error when required references or publication fields are invalid.
pub fn validate(
    paths: &EnvironmentPaths,
    selector: Option<&str>,
) -> Result<ValidationReport, String> {
    let catalog = discover(paths)?;
    let selected = select_artifacts(&catalog, selector)?;
    let mut diagnostics = Vec::new();
    let mut errors = Vec::new();

    for artifact in &selected {
        let relationships = artifact
            .dependencies()
            .iter()
            .map(|dependency| dependency.relationship().to_owned())
            .collect::<Vec<_>>();
        if let Err(error) =
            validate_source_relationship_cardinality(artifact.kind(), &relationships)
        {
            errors.push(format!("{}: {error}", artifact.id()));
        }
        let mut seen_dependencies = BTreeSet::new();
        for dependency in artifact.dependencies() {
            if !seen_dependencies.insert((dependency.relationship(), dependency.id())) {
                errors.push(format!(
                    "{} declares duplicate {} reference '{}'",
                    artifact.id(),
                    dependency.relationship(),
                    dependency.id()
                ));
                continue;
            }
            let requirement = match dependency
                .version()
                .map(semver::VersionReq::parse)
                .transpose()
            {
                Ok(requirement) => requirement,
                Err(error) => {
                    errors.push(format!(
                        "{} has an invalid version requirement for {}: {error}",
                        artifact.id(),
                        dependency.id()
                    ));
                    continue;
                }
            };
            let local_child = find_local_source(&catalog, dependency.id(), requirement.as_ref());
            if let Some(child) = local_child {
                if let Err(error) =
                    validate_dependency_edge(artifact.kind(), dependency.relationship(), child)
                {
                    errors.push(error);
                }
            } else if !dependency.optional() && dependency.workshop_id().is_none() {
                errors.push(format!(
                    "{} requires missing {} reference '{}' at version {}",
                    artifact.id(),
                    dependency.relationship(),
                    dependency.id(),
                    dependency.version().unwrap_or("any")
                ));
            } else if dependency.optional() {
                diagnostics.push(format!(
                    "{} has optional {} reference '{}' outside this workspace",
                    artifact.id(),
                    dependency.relationship(),
                    dependency.id()
                ));
            }
            if dependency.optional()
                && matches!(
                    (artifact.kind(), dependency.relationship()),
                    (ContentKind::Packagepack, "engine" | "enginepack")
                        | (ContentKind::Game, "engine")
                        | (ContentKind::Enginepack, "engine")
                        | (ContentKind::Gamepack, "game")
                )
            {
                errors.push(format!(
                    "{} cannot mark its required '{}' relationship optional",
                    artifact.id(),
                    dependency.relationship()
                ));
            }
        }
        for conflict in artifact.conflicts() {
            if conflict.id() == artifact.id() {
                errors.push(format!("{} conflicts with itself", artifact.id()));
            }
        }
        if artifact.workshop().app_id().is_none() {
            diagnostics.push(format!(
                "{} has no [{}.steam].app-id; local lifecycle is available but Workshop publish is not",
                artifact.id(),
                artifact.kind()
            ));
        }
    }

    if errors.is_empty() {
        Ok(ValidationReport {
            checked: selected
                .iter()
                .map(|artifact| artifact.id().to_owned())
                .collect(),
            diagnostics,
        })
    } else {
        Err(errors.join("\n"))
    }
}

/// Stage a source content artifact as a Vapor package.
///
/// # Errors
///
/// Returns discovery, validation, or filesystem errors.
pub fn package(
    paths: &EnvironmentPaths,
    selector: &str,
    dry_run: bool,
) -> Result<PackageReport, String> {
    package_for_target(paths, selector, dry_run, None)
}

/// Stage a source content artifact for one runtime target.
///
/// # Errors
///
/// Returns discovery, validation, runtime-output, or filesystem errors.
pub fn package_for_target(
    paths: &EnvironmentPaths,
    selector: &str,
    dry_run: bool,
    target_triple: Option<&str>,
) -> Result<PackageReport, String> {
    let targets = target_triple
        .map(|target| vec![target.to_owned()])
        .unwrap_or_default();
    package_for_targets(paths, selector, dry_run, &targets)
}

/// Stage a source content artifact for one or more runtime targets.
///
/// An empty target list uses the host target and Cargo's host-default output
/// directory. Non-empty target lists expect Cargo's target-specific output
/// directories.
///
/// # Errors
///
/// Returns discovery, validation, runtime-output, or filesystem errors.
pub fn package_for_targets(
    paths: &EnvironmentPaths,
    selector: &str,
    dry_run: bool,
    target_triples: &[String],
) -> Result<PackageReport, String> {
    let runtime_targets = RuntimeTarget::many(target_triples)?;
    package_with_runtime_targets(paths, selector, dry_run, runtime_targets)
}

fn package_with_runtime_targets(
    paths: &EnvironmentPaths,
    selector: &str,
    dry_run: bool,
    runtime_targets: Vec<RuntimeTarget>,
) -> Result<PackageReport, String> {
    let catalog = discover(paths)?;
    let artifact = catalog.find(selector).ok_or_else(|| {
        format!(
            "unknown content artifact '{selector}'\nhelp: inspect source content with `content list`"
        )
    })?;
    validate(paths, Some(artifact.id()))?;
    let layout = ContentLayout::new(paths.installation());
    let package_root = layout.output_packages().join(slug(artifact.id()));
    let runtime_target_label = runtime_target_label(&runtime_targets);

    if dry_run {
        let temporary_root = TemporaryDirectory::new("vapor-content-package", artifact.id())?;
        stage_deployed_artifact(paths, temporary_root.path(), artifact, &runtime_targets)?;
        let fingerprint = fingerprint_tree(temporary_root.path())?;
        return Ok(PackageReport {
            artifact_id: artifact.id().to_owned(),
            root: package_root,
            fingerprint,
            receipt: None,
            dry_run: true,
            runtime_target: runtime_target_label,
        });
    }

    reset_directory(paths.installation().root(), &package_root)?;
    stage_deployed_artifact(paths, &package_root, artifact, &runtime_targets)?;
    let fingerprint = fingerprint_tree(&package_root)?;
    let receipt = write_receipt(
        paths.installation(),
        "package",
        artifact.id(),
        "staged",
        Some(&format!(
            "package={};target={runtime_target_label}",
            package_root.display()
        )),
    )?;
    Ok(PackageReport {
        artifact_id: artifact.id().to_owned(),
        root: package_root,
        fingerprint,
        receipt: Some(receipt),
        dry_run: false,
        runtime_target: runtime_target_label,
    })
}

/// Acquire content into the app-owned cache.
///
/// Local source artifacts are packaged and copied into the cache. Numeric
/// Workshop IDs install from an existing cache until a live SteamUGC provider is
/// available in the running app.
///
/// # Errors
///
/// Returns provider, discovery, package, or filesystem errors.
pub fn acquire(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
    account: Option<&str>,
) -> Result<AcquireReport, String> {
    let layout = ContentLayout::new(installation);
    if is_published_file_id(selector) && account.is_some_and(|value| !value.trim().is_empty()) {
        let app_id = resolve_download_app_id(installation, paths, selector)?;
        return acquire_workshop_item(installation, app_id, selector, account);
    }

    if let Some(paths) = paths {
        let catalog = discover(paths)?;
        if let Some(artifact) = catalog.find(selector) {
            let package = package(paths, artifact.id(), false)?;
            let cache_key = artifact
                .workshop()
                .published_file_id()
                .map_or_else(|| slug(artifact.id()), ToOwned::to_owned);
            let cache_root = layout.cache().join("packages").join(cache_key);
            reset_directory(installation.root(), &cache_root)?;
            copy_tree(package.root(), &cache_root, package.root(), &[])?;
            let receipt = write_receipt(
                installation,
                "acquire",
                artifact.id(),
                "cached",
                Some(&format!("cache={}", cache_root.display())),
            )?;
            let mut index = load_index(&layout)?;
            index.caches.insert(
                artifact.id().to_owned(),
                CacheRecord {
                    artifact_id: artifact.id().to_owned(),
                    workshop_id: artifact
                        .workshop()
                        .published_file_id()
                        .map(ToOwned::to_owned),
                    version: artifact.version().map(ToOwned::to_owned),
                    cache_root: cache_root.clone(),
                    fingerprint: package.fingerprint().clone(),
                    acquired_at: now_seconds(),
                },
            );
            save_index(&layout, &index)?;
            return Ok(AcquireReport {
                artifact_id: artifact.id().to_owned(),
                cache_root,
                fingerprint: package.fingerprint().clone(),
                receipt,
            });
        }
    }

    let index = load_index(&layout)?;
    if let Some((artifact_id, cache)) = index.cache_by_selector(selector) {
        let receipt = write_receipt(
            installation,
            "acquire",
            &artifact_id,
            "cached-already",
            Some(&format!("cache={}", cache.cache_root.display())),
        )?;
        return Ok(AcquireReport {
            artifact_id,
            cache_root: cache.cache_root.clone(),
            fingerprint: cache.fingerprint.clone(),
            receipt,
        });
    }

    if let Some(seed) = root_content_seed(installation, selector)? {
        return acquire_workshop_item(installation, seed.app_id, &seed.workshop_id, account);
    }

    if is_published_file_id(selector) {
        let app_id = resolve_download_app_id(installation, paths, selector)?;
        return acquire_workshop_item(installation, app_id, selector, account);
    }

    Err(format!(
        "cannot acquire Workshop item '{selector}': no matching source artifact, app-owned cache entry, root content seed, or numeric PublishedFileId exists\nhelp: acquire from an open source artifact, add a [[root.content]] seed, or pass a PublishedFileId"
    ))
}

/// Acquire one or more content items into the app-owned cache.
///
/// Numeric PublishedFileIds are downloaded through one SteamCMD provider
/// session. Mixed source/cache selectors fall back to the single-item
/// acquisition path.
///
/// # Errors
///
/// Returns provider, discovery, package, or filesystem errors.
pub fn acquire_many(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selectors: &[String],
    account: Option<&str>,
) -> Result<Vec<AcquireReport>, String> {
    if selectors.is_empty() {
        return Err("at least one content target is required".to_owned());
    }
    if selectors
        .iter()
        .all(|selector| is_published_file_id(selector))
    {
        let downloads = selectors
            .iter()
            .map(|selector| {
                Ok(WorkshopDownload {
                    app_id: resolve_download_app_id(installation, paths, selector)?,
                    published_file_id: selector.to_owned(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        return acquire_workshop_items(installation, &downloads, account);
    }
    selectors
        .iter()
        .map(|selector| acquire(installation, paths, selector, account))
        .collect()
}

/// Download one or more content items into the app-owned cache.
///
/// An explicit account means the caller wants a live provider refresh when the
/// selector can be resolved to a Workshop item. Other selectors fall back to
/// the ordinary acquisition path.
///
/// # Errors
///
/// Returns provider, discovery, package, or filesystem errors.
pub fn download_many(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selectors: &[String],
    account: Option<&str>,
) -> Result<Vec<AcquireReport>, String> {
    if selectors.is_empty() {
        return Err("at least one content target is required".to_owned());
    }
    if account.unwrap_or("").trim().is_empty() {
        return acquire_many(installation, paths, selectors, account);
    }

    let index = load_index(&ContentLayout::new(installation))?;
    let mut downloads = Vec::new();
    let mut fallback = Vec::new();
    let mut seen = BTreeSet::new();
    for selector in selectors {
        if let Some(download) =
            resolve_live_workshop_download(installation, paths, &index, selector)?
        {
            if seen.insert((download.app_id, download.published_file_id.clone())) {
                downloads.push(download);
            }
        } else {
            fallback.push(selector);
        }
    }

    let mut reports = Vec::new();
    if !downloads.is_empty() {
        reports.extend(acquire_workshop_items(installation, &downloads, account)?);
    }
    for selector in fallback {
        reports.push(acquire(installation, paths, selector, account)?);
    }
    Ok(reports)
}

fn acquire_workshop_item(
    installation: &InstallationPaths,
    app_id: u32,
    published_file_id: &str,
    account: Option<&str>,
) -> Result<AcquireReport, String> {
    let account = workshop_download_account(account);
    let provider_root =
        run_steamcmd_workshop_download(installation, app_id, published_file_id, &account)?;
    import_workshop_item(installation, app_id, published_file_id, &provider_root)
}

fn acquire_workshop_items(
    installation: &InstallationPaths,
    downloads: &[WorkshopDownload],
    account: Option<&str>,
) -> Result<Vec<AcquireReport>, String> {
    let account = workshop_download_account(account);
    let provider_roots = run_steamcmd_workshop_downloads(installation, downloads, &account)?;
    downloads
        .iter()
        .zip(provider_roots.iter())
        .map(|(download, provider_root)| {
            import_workshop_item(
                installation,
                download.app_id,
                &download.published_file_id,
                provider_root,
            )
        })
        .collect()
}

fn workshop_download_account(account: Option<&str>) -> String {
    let account = account.unwrap_or("").trim();
    if account.is_empty() {
        "anonymous".to_owned()
    } else {
        account.to_owned()
    }
}

fn import_workshop_item(
    installation: &InstallationPaths,
    app_id: u32,
    published_file_id: &str,
    provider_root: &Path,
) -> Result<AcquireReport, String> {
    let layout = ContentLayout::new(installation);
    let observed_root = layout
        .steam_downloads()
        .join(app_id.to_string())
        .join(published_file_id);
    reset_directory(installation.root(), &observed_root)?;
    copy_tree(provider_root, &observed_root, provider_root, &[])?;
    let artifact = read_deployed_manifest(&observed_root)?;
    let cache_root = layout.cache().join("packages").join(published_file_id);
    reset_directory(installation.root(), &cache_root)?;
    copy_tree(&observed_root, &cache_root, &observed_root, &[])?;
    let fingerprint = fingerprint_tree(&cache_root)?;
    let mut index = load_index(&layout)?;
    index.caches.insert(
        artifact.id().to_owned(),
        CacheRecord {
            artifact_id: artifact.id().to_owned(),
            workshop_id: Some(published_file_id.to_owned()),
            version: artifact.version().map(ToOwned::to_owned),
            cache_root: cache_root.clone(),
            fingerprint: fingerprint.clone(),
            acquired_at: now_seconds(),
        },
    );
    save_index(&layout, &index)?;
    let receipt = write_receipt(
        installation,
        "download",
        artifact.id(),
        "downloaded",
        Some(&format!(
            "workshop-id={published_file_id}; observed={}; cache={}",
            observed_root.display(),
            cache_root.display()
        )),
    )?;
    Ok(AcquireReport {
        artifact_id: artifact.id().to_owned(),
        cache_root,
        fingerprint,
        receipt,
    })
}

/// Import a Workshop download supplied by Steamworks into Vapor's app-owned cache.
///
/// # Errors
///
/// Returns validation, package, or filesystem errors when the downloaded item
/// cannot be copied and indexed as deployed content.
pub fn import_workshop_item_from_provider(
    installation: &InstallationPaths,
    app_id: u32,
    published_file_id: &str,
    provider_root: &Path,
) -> Result<AcquireReport, String> {
    import_workshop_item(installation, app_id, published_file_id, provider_root)
}

/// Install source or cached content into the app-owned installed-content tree.
///
/// # Errors
///
/// Returns dependency, conflict, provider, verification, or filesystem errors.
pub fn install(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
) -> Result<Vec<InstallReport>, String> {
    install_with_account(installation, paths, selector, None)
}

/// Install source or cached content for one runtime target.
///
/// # Errors
///
/// Returns dependency, conflict, provider, verification, or filesystem errors.
pub fn install_for_target(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
    target_triple: Option<&str>,
) -> Result<Vec<InstallReport>, String> {
    install_with_account_for_target(installation, paths, selector, None, target_triple)
}

/// Install source or cached content for one or more runtime targets.
///
/// # Errors
///
/// Returns dependency, conflict, provider, verification, or filesystem errors.
pub fn install_for_targets(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
    target_triples: &[String],
) -> Result<Vec<InstallReport>, String> {
    install_with_account_for_targets(installation, paths, selector, None, target_triples)
}

/// Install source or cached content, downloading Workshop dependencies with an account when needed.
///
/// # Errors
///
/// Returns dependency, conflict, provider, verification, or filesystem errors.
pub fn install_with_account(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
    account: Option<&str>,
) -> Result<Vec<InstallReport>, String> {
    install_with_account_for_target(installation, paths, selector, account, None)
}

/// Install source or cached content with an explicit runtime target.
///
/// # Errors
///
/// Returns dependency, conflict, provider, verification, or filesystem errors.
pub fn install_with_account_for_target(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
    account: Option<&str>,
    target_triple: Option<&str>,
) -> Result<Vec<InstallReport>, String> {
    let targets = target_triple
        .map(|target| vec![target.to_owned()])
        .unwrap_or_default();
    install_with_account_for_targets(installation, paths, selector, account, &targets)
}

/// Install source or cached content with explicit runtime targets.
///
/// # Errors
///
/// Returns dependency, conflict, provider, verification, or filesystem errors.
pub fn install_with_account_for_targets(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
    account: Option<&str>,
    target_triples: &[String],
) -> Result<Vec<InstallReport>, String> {
    let runtime_targets = RuntimeTarget::many(target_triples)?;
    let layout = ContentLayout::new(installation);
    let catalog = paths.map(discover).transpose()?;
    let mut index = load_index(&layout)?;
    let mut reports = Vec::new();
    let mut visiting = BTreeSet::new();
    let context = InstallContext {
        installation,
        paths,
        catalog: catalog.as_ref(),
        layout: &layout,
        account,
        runtime_targets: &runtime_targets,
    };
    let mut state = InstallState {
        index: &mut index,
        visiting: &mut visiting,
        reports: &mut reports,
    };
    install_selector(
        &context,
        InstallSelectorRequest {
            selector,
            expected_id: None,
            parent_kind: None,
            relationship: None,
            require_workspace: false,
        },
        &mut state,
    )?;
    save_index(&layout, &index)?;
    Ok(reports)
}

/// Update installed content by reinstalling it from source or cache.
///
/// # Errors
///
/// Returns the same errors as [`install`].
pub fn update(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: Option<&str>,
) -> Result<Vec<InstallReport>, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    let targets = if let Some(selector) = selector {
        vec![resolve_installed_selector(&index, selector)?]
    } else {
        index.installed.keys().cloned().collect()
    };
    if targets.is_empty() {
        return Err("no installed content to update".to_owned());
    }
    let mut reports = Vec::new();
    for target in targets {
        reports.extend(install(installation, paths, &target)?);
    }
    Ok(reports)
}

/// Disable installed content without deleting its artifact root.
///
/// # Errors
///
/// Returns an error when the target is unknown or the move fails.
pub fn disable(installation: &InstallationPaths, selector: &str) -> Result<InstallReport, String> {
    move_enabled_state(installation, selector, false)
}

/// Enable disabled content without reinstalling it.
///
/// # Errors
///
/// Returns an error when the target is unknown or the move fails.
pub fn enable(installation: &InstallationPaths, selector: &str) -> Result<InstallReport, String> {
    move_enabled_state(installation, selector, true)
}

/// Uninstall content and remove generated installed-state records.
///
/// # Errors
///
/// Returns an error when generated state cannot be updated.
pub fn uninstall(
    installation: &InstallationPaths,
    selector: &str,
) -> Result<UninstallReport, String> {
    let layout = ContentLayout::new(installation);
    let mut index = load_index(&layout)?;
    let artifact_id = resolve_installed_selector(&index, selector)?;
    let mut removed = false;
    for root in [
        layout.installed().join(&artifact_id),
        layout.disabled().join(&artifact_id),
    ] {
        if root.exists() {
            remove_directory(installation.root(), &root)?;
            removed = true;
        }
    }
    index.installed.remove(&artifact_id);
    save_index(&layout, &index)?;
    if current_selection(installation)?
        .as_ref()
        .is_some_and(|selection| selection.artifact_id() == artifact_id)
    {
        clear_selection(installation)?;
    }
    let receipt = write_receipt(
        installation,
        "uninstall",
        &artifact_id,
        if removed { "removed" } else { "not-present" },
        None,
    )?;
    Ok(UninstallReport {
        artifact_id,
        removed,
        receipt,
    })
}

/// Select an installed packagepack for play.
///
/// # Errors
///
/// Returns an error when the target is not installed, not enabled, not a
/// packagepack, or the selection cannot be written.
pub fn select_packagepack(
    installation: &InstallationPaths,
    selector: &str,
) -> Result<PackagepackSelection, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    let artifact_id = resolve_installed_selector(&index, selector)?;
    let record = index
        .installed
        .get(&artifact_id)
        .ok_or_else(|| format!("content is not installed: {artifact_id}"))?;
    if record.kind != ContentKind::Packagepack.to_string() {
        return Err(format!(
            "selected content must be a packagepack: {} is {}",
            artifact_id, record.kind
        ));
    }
    if !record.enabled {
        return Err(format!("cannot select disabled packagepack: {artifact_id}"));
    }
    if !record.installed_root.exists() {
        return Err(format!(
            "cannot select packagepack with missing artifact root: {}",
            record.installed_root.display()
        ));
    }
    let selection = PackagepackSelection {
        schema_version: CONTENT_STATE_SCHEMA,
        artifact_id: artifact_id.clone(),
        installed_root: record.installed_root.clone(),
        fingerprint: record.fingerprint.clone(),
        selected_at: now_seconds(),
    };
    if let Some(parent) = layout.selection().parent() {
        fs::create_dir_all(parent).map_err(io("create content selection parent", parent))?;
    }
    let encoded = toml::to_string_pretty(&selection)
        .map_err(|error| format!("failed to encode packagepack selection: {error}"))?;
    fs::write(layout.selection(), encoded)
        .map_err(io("write packagepack selection", layout.selection()))?;
    write_receipt(
        installation,
        "select",
        &artifact_id,
        "selected",
        Some(&format!("root={}", selection.installed_root.display())),
    )?;
    Ok(selection)
}

/// Read the current selected packagepack.
///
/// # Errors
///
/// Returns filesystem or TOML parse errors.
pub fn current_selection(
    installation: &InstallationPaths,
) -> Result<Option<PackagepackSelection>, String> {
    let layout = ContentLayout::new(installation);
    if !layout.selection().is_file() {
        return Ok(None);
    }
    let source = fs::read_to_string(layout.selection())
        .map_err(io("read packagepack selection", layout.selection()))?;
    toml::from_str(&source).map(Some).map_err(|error| {
        format!(
            "failed to parse '{}': {error}",
            layout.selection().display()
        )
    })
}

/// Resolve an app manifest launch alias to its packagepack content ID.
///
/// # Errors
///
/// Returns an error when the app manifest has no matching launch alias or the
/// alias does not point at a packagepack seed.
pub fn default_packagepack_id(
    installation: &InstallationPaths,
    launch: &str,
) -> Result<String, String> {
    let seed = root_content_seed(installation, launch)?
        .ok_or_else(|| format!("app manifest has no default packagepack launch '{launch}'"))?;
    let expected_kind = ContentKind::Packagepack.to_string();
    if seed.kind != expected_kind {
        return Err(format!(
            "app manifest default launch '{launch}' must point at a packagepack, but '{}' is {}",
            seed.id, seed.kind
        ));
    }
    Ok(seed.id)
}

/// Clear the selected packagepack.
///
/// # Errors
///
/// Returns filesystem errors.
pub fn clear_selection(installation: &InstallationPaths) -> Result<(), String> {
    let layout = ContentLayout::new(installation);
    if layout.selection().exists() {
        fs::remove_file(layout.selection())
            .map_err(io("remove packagepack selection", layout.selection()))?;
    }
    write_receipt(installation, "deselect", "packagepack", "cleared", None)?;
    Ok(())
}

/// Verify installed content against app-owned receipts and indexes.
///
/// # Errors
///
/// Returns an error when state cannot be read.
pub fn verify(
    installation: &InstallationPaths,
    selector: Option<&str>,
) -> Result<Vec<VerifyReport>, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    let targets = if let Some(selector) = selector {
        vec![resolve_installed_selector(&index, selector)?]
    } else {
        index.installed.keys().cloned().collect()
    };
    Ok(targets
        .into_iter()
        .map(|artifact_id| verify_one(&index, &artifact_id))
        .collect())
}

/// Repair corrupted installed content by reinstalling from source or cache.
///
/// # Errors
///
/// Returns provider or filesystem errors when repair cannot proceed.
pub fn repair(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: Option<&str>,
) -> Result<Vec<InstallReport>, String> {
    let layout = ContentLayout::new(installation);
    let reports = verify(installation, selector)?;
    let mut repaired = Vec::new();
    for report in reports {
        if report.ok() {
            continue;
        }
        let corrupt = layout.installed().join(report.artifact_id());
        if corrupt.exists() {
            let quarantine = layout.quarantine().join(format!(
                "{}-{}",
                slug(report.artifact_id()),
                now_seconds()
            ));
            if let Some(parent) = quarantine.parent() {
                fs::create_dir_all(parent).map_err(io("create quarantine parent", parent))?;
            }
            fs::rename(&corrupt, &quarantine).map_err(io("quarantine content", &corrupt))?;
            write_receipt(
                installation,
                "repair",
                report.artifact_id(),
                "quarantined",
                Some(&format!("quarantine={}", quarantine.display())),
            )?;
        }
        repaired.extend(install(installation, paths, report.artifact_id())?);
    }
    if repaired.is_empty() {
        let target = selector.unwrap_or("all");
        write_receipt(installation, "repair", target, "already-ok", None)?;
    }
    Ok(repaired)
}

/// Write a Workshop create preview receipt.
///
/// Real item creation requires a live SteamUGC provider and remains a manual
/// authority-changing operation.
///
/// # Errors
///
/// Returns discovery or receipt errors.
pub fn create_workshop_item(
    paths: &EnvironmentPaths,
    selector: &str,
    account: Option<&str>,
    dry_run: bool,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    create_workshop_item_for_target(paths, selector, account, None, dry_run, confirmed)
}

/// Write or upload a Workshop create operation with an explicit runtime target.
///
/// # Errors
///
/// Returns discovery, packaging, provider, or receipt errors.
pub fn create_workshop_item_for_target(
    paths: &EnvironmentPaths,
    selector: &str,
    account: Option<&str>,
    target_triple: Option<&str>,
    dry_run: bool,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    let targets = target_triple
        .map(|target| vec![target.to_owned()])
        .unwrap_or_default();
    create_workshop_item_for_targets(paths, selector, account, &targets, dry_run, confirmed)
}

/// Write or upload a Workshop create operation with explicit runtime targets.
///
/// # Errors
///
/// Returns discovery, packaging, provider, or receipt errors.
pub fn create_workshop_item_for_targets(
    paths: &EnvironmentPaths,
    selector: &str,
    account: Option<&str>,
    target_triples: &[String],
    dry_run: bool,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    create_workshop_item_for_targets_checked(
        paths,
        selector,
        account,
        target_triples,
        dry_run,
        confirmed,
        None,
    )
}

/// Create a Workshop item only if the package staged for upload matches the
/// exact fingerprint previously reviewed by the caller.
pub fn create_workshop_item_reviewed(
    paths: &EnvironmentPaths,
    selector: &str,
    expected_fingerprint: &str,
    account: &str,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    create_workshop_item_for_targets_checked(
        paths,
        selector,
        Some(account),
        &[],
        false,
        confirmed,
        Some(expected_fingerprint),
    )
}

fn create_workshop_item_for_targets_checked(
    paths: &EnvironmentPaths,
    selector: &str,
    account: Option<&str>,
    target_triples: &[String],
    dry_run: bool,
    confirmed: bool,
    expected_fingerprint: Option<&str>,
) -> Result<WorkshopOperationReport, String> {
    let catalog = discover(paths)?;
    let artifact = catalog.find(selector).ok_or_else(|| {
        format!("unknown content artifact '{selector}'\nhelp: inspect source content with `content list`")
    })?;
    if artifact.workshop().app_id().is_none() {
        return Err(format!(
            "{} has no [{}.steam].app-id and cannot be created on Workshop",
            artifact.id(),
            artifact.kind()
        ));
    }
    if !dry_run && artifact.workshop().published_file_id().is_some() {
        return Err(format!(
            "{} already has PublishedFileId {}; use `content publish` for updates",
            artifact.id(),
            artifact.workshop().published_file_id().expect("checked")
        ));
    }
    if !dry_run && account.unwrap_or("").trim().is_empty() {
        return Err(
            "real Workshop creation requires --account ACCOUNT after reviewing --dry-run"
                .to_owned(),
        );
    }
    if !dry_run && !confirmed {
        return Err("real Workshop creation requires --yes after reviewing --dry-run".to_owned());
    }

    let package = package_for_targets(paths, artifact.id(), false, target_triples)?;
    let (upload_root, _reviewed_stage, package_fingerprint) =
        if let Some(expected) = expected_fingerprint {
            let staged = TemporaryDirectory::new("vapor-workshop-reviewed", artifact.id())?;
            copy_tree(package.root(), staged.path(), package.root(), &[])?;
            let fingerprint = fingerprint_tree(staged.path())?;
            let fingerprint = format!("{}:{}", fingerprint.algorithm(), fingerprint.hash());
            if fingerprint != expected {
                return Err(format!(
                    "{} changed after review; stage a fresh Workshop review before uploading",
                    artifact.id()
                ));
            }
            let upload_root = staged.path().to_path_buf();
            (upload_root, Some(staged), fingerprint)
        } else {
            let fingerprint = format!(
                "{}:{}",
                package.fingerprint().algorithm(),
                package.fingerprint().hash()
            );
            (package.root().to_path_buf(), None, fingerprint)
        };
    let script = write_workshop_script(paths, artifact, &upload_root, None, dry_run)?;
    if dry_run {
        let receipt = write_receipt(
            paths.installation(),
            "workshop-create",
            artifact.id(),
            "dry-run",
            Some(&format!(
                "script={}; fingerprint={package_fingerprint}",
                script.display()
            )),
        )?;
        return Ok(WorkshopOperationReport {
            artifact_id: artifact.id().to_owned(),
            script: Some(script),
            receipt,
            uploaded: false,
            published_file_id: None,
        });
    }

    run_steamcmd_workshop_build(paths, account.expect("account checked"), &script)?;
    let published_file_id = read_workshop_script_published_file_id(&script)?;
    if published_file_id == "0" {
        return Err(format!(
            "SteamCMD did not write a PublishedFileId into '{}'",
            script.display()
        ));
    }
    record_published_file_id(artifact, &published_file_id)?;
    let receipt = write_receipt(
        paths.installation(),
        "workshop-create",
        artifact.id(),
        "created",
        Some(&format!(
            "script={}; published-file-id={}; fingerprint={package_fingerprint}",
            script.display(),
            published_file_id
        )),
    )?;
    Ok(WorkshopOperationReport {
        artifact_id: artifact.id().to_owned(),
        script: Some(script),
        receipt,
        uploaded: true,
        published_file_id: Some(published_file_id),
    })
}

/// Publish or preview a Workshop item update.
///
/// Dry-runs write the package and SteamCMD Workshop VDF. Real updates require
/// `account` and `confirmed` and use the controlled SteamCMD provider.
///
/// # Errors
///
/// Returns package, provider, SteamCMD, or authority errors.
pub fn publish_workshop_item(
    paths: &EnvironmentPaths,
    selector: &str,
    account: Option<&str>,
    change_note: Option<&str>,
    dry_run: bool,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    publish_workshop_item_for_target(
        paths,
        selector,
        account,
        None,
        change_note,
        dry_run,
        confirmed,
    )
}

/// Publish or preview a Workshop item update for one runtime target.
///
/// # Errors
///
/// Returns package, provider, SteamCMD, or authority errors.
pub fn publish_workshop_item_for_target(
    paths: &EnvironmentPaths,
    selector: &str,
    account: Option<&str>,
    target_triple: Option<&str>,
    change_note: Option<&str>,
    dry_run: bool,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    let targets = target_triple
        .map(|target| vec![target.to_owned()])
        .unwrap_or_default();
    publish_workshop_item_for_targets(
        paths,
        selector,
        account,
        &targets,
        change_note,
        dry_run,
        confirmed,
    )
}

/// Publish or preview a Workshop item update for runtime targets.
///
/// # Errors
///
/// Returns package, provider, SteamCMD, or authority errors.
pub fn publish_workshop_item_for_targets(
    paths: &EnvironmentPaths,
    selector: &str,
    account: Option<&str>,
    target_triples: &[String],
    change_note: Option<&str>,
    dry_run: bool,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    publish_workshop_items_for_targets(
        paths,
        &[selector.to_owned()],
        account,
        target_triples,
        change_note,
        dry_run,
        confirmed,
    )
    .and_then(|mut reports| {
        reports
            .pop()
            .ok_or_else(|| "no Workshop item was published".to_owned())
    })
}

/// Publish or preview one or more Workshop item updates.
///
/// Multiple real uploads are sent through one SteamCMD process so the authority
/// boundary remains a single interactive provider session.
///
/// # Errors
///
/// Returns discovery, packaging, receipt, or provider errors.
pub fn publish_workshop_items(
    paths: &EnvironmentPaths,
    selectors: &[String],
    account: Option<&str>,
    change_note: Option<&str>,
    dry_run: bool,
    confirmed: bool,
) -> Result<Vec<WorkshopOperationReport>, String> {
    publish_workshop_items_for_target(
        paths,
        selectors,
        account,
        None,
        change_note,
        dry_run,
        confirmed,
    )
}

/// Publish or preview one or more Workshop item updates for one runtime target.
///
/// Multiple real uploads are sent through one SteamCMD process so the authority
/// boundary remains a single interactive provider session.
///
/// # Errors
///
/// Returns discovery, packaging, receipt, or provider errors.
pub fn publish_workshop_items_for_target(
    paths: &EnvironmentPaths,
    selectors: &[String],
    account: Option<&str>,
    target_triple: Option<&str>,
    change_note: Option<&str>,
    dry_run: bool,
    confirmed: bool,
) -> Result<Vec<WorkshopOperationReport>, String> {
    let targets = target_triple
        .map(|target| vec![target.to_owned()])
        .unwrap_or_default();
    publish_workshop_items_for_targets(
        paths,
        selectors,
        account,
        &targets,
        change_note,
        dry_run,
        confirmed,
    )
}

/// Publish or preview one or more Workshop item updates for runtime targets.
///
/// Multiple real uploads are sent through one SteamCMD process so the authority
/// boundary remains a single interactive provider session.
///
/// # Errors
///
/// Returns discovery, packaging, receipt, or provider errors.
pub fn publish_workshop_items_for_targets(
    paths: &EnvironmentPaths,
    selectors: &[String],
    account: Option<&str>,
    target_triples: &[String],
    change_note: Option<&str>,
    dry_run: bool,
    confirmed: bool,
) -> Result<Vec<WorkshopOperationReport>, String> {
    publish_workshop_items_for_targets_checked(
        paths,
        WorkshopPublishOptions {
            selectors,
            account,
            target_triples,
            change_note,
            dry_run,
            confirmed,
            expected_fingerprint: None,
        },
    )
}

/// Update one Workshop item only if the package staged for upload matches the
/// exact fingerprint previously reviewed by the caller.
pub fn publish_workshop_item_reviewed(
    paths: &EnvironmentPaths,
    selector: &str,
    expected_fingerprint: &str,
    account: &str,
    change_note: Option<&str>,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    let mut reports = publish_workshop_items_for_targets_checked(
        paths,
        WorkshopPublishOptions {
            selectors: &[selector.to_owned()],
            account: Some(account),
            target_triples: &[],
            change_note,
            dry_run: false,
            confirmed,
            expected_fingerprint: Some((selector, expected_fingerprint)),
        },
    )?;
    reports
        .pop()
        .ok_or_else(|| "no Workshop item was published".to_owned())
}

struct WorkshopPublishOptions<'a> {
    selectors: &'a [String],
    account: Option<&'a str>,
    target_triples: &'a [String],
    change_note: Option<&'a str>,
    dry_run: bool,
    confirmed: bool,
    expected_fingerprint: Option<(&'a str, &'a str)>,
}

fn publish_workshop_items_for_targets_checked(
    paths: &EnvironmentPaths,
    options: WorkshopPublishOptions<'_>,
) -> Result<Vec<WorkshopOperationReport>, String> {
    let WorkshopPublishOptions {
        selectors,
        account,
        target_triples,
        change_note,
        dry_run,
        confirmed,
        expected_fingerprint,
    } = options;
    if selectors.is_empty() {
        return Err("at least one Workshop artifact is required".to_owned());
    }
    let catalog = discover(paths)?;
    if !dry_run && account.unwrap_or("").trim().is_empty() {
        return Err(
            "real Workshop publication requires --account ACCOUNT after reviewing --dry-run"
                .to_owned(),
        );
    }
    if !dry_run && !confirmed {
        return Err(
            "real Workshop publication requires --yes after reviewing --dry-run".to_owned(),
        );
    }

    let mut staged = Vec::new();
    let mut reviewed_stages = Vec::new();
    for selector in selectors {
        let artifact = catalog.find(selector).ok_or_else(|| {
            format!(
                "unknown content artifact '{selector}'\nhelp: inspect source content with `content list`"
            )
        })?;
        if artifact.workshop().app_id().is_none() {
            return Err(format!(
                "{} has no [{}.steam].app-id and cannot be published",
                artifact.id(),
                artifact.kind()
            ));
        }
        if artifact.workshop().published_file_id().is_none() && !dry_run {
            return Err(format!(
                "{} has no PublishedFileId; run `content create {} --dry-run` and create the item manually before a real update",
                artifact.id(),
                artifact.name()
            ));
        }

        let package = package_for_targets(paths, artifact.id(), false, target_triples)?;
        let mut package_fingerprint = format!(
            "{}:{}",
            package.fingerprint().algorithm(),
            package.fingerprint().hash()
        );
        let mut upload_root = package.root().to_path_buf();
        if let Some((reviewed_selector, expected)) = expected_fingerprint
            && selector.as_str() == reviewed_selector
        {
            let reviewed = TemporaryDirectory::new("vapor-workshop-reviewed", artifact.id())?;
            copy_tree(package.root(), reviewed.path(), package.root(), &[])?;
            let fingerprint = fingerprint_tree(reviewed.path())?;
            let fingerprint = format!("{}:{}", fingerprint.algorithm(), fingerprint.hash());
            if fingerprint != expected {
                return Err(format!(
                    "{} changed after review; stage a fresh Workshop review before uploading",
                    artifact.id()
                ));
            }
            package_fingerprint = fingerprint;
            upload_root = reviewed.path().to_path_buf();
            reviewed_stages.push(reviewed);
        }
        let script = write_workshop_script(paths, artifact, &upload_root, change_note, dry_run)?;
        staged.push((
            artifact.id().to_owned(),
            script,
            artifact
                .workshop()
                .published_file_id()
                .map(ToOwned::to_owned),
            package_fingerprint,
        ));
    }

    if !dry_run {
        let scripts = staged
            .iter()
            .map(|(_, script, _, _)| script.clone())
            .collect::<Vec<_>>();
        run_steamcmd_workshop_builds(paths, account.expect("account checked"), &scripts)?;
    }

    let receipt_status = if dry_run { "dry-run" } else { "uploaded" };
    let mut reports = Vec::new();
    for (artifact_id, script, published_file_id, fingerprint) in staged {
        let receipt = write_receipt(
            paths.installation(),
            "workshop-publish",
            &artifact_id,
            receipt_status,
            Some(&format!(
                "script={}; fingerprint={fingerprint}",
                script.display()
            )),
        )?;
        reports.push(WorkshopOperationReport {
            artifact_id,
            script: Some(script),
            receipt,
            uploaded: !dry_run,
            published_file_id,
        });
    }
    Ok(reports)
}

/// Preview a Workshop delete operation.
///
/// Deletion needs a live SteamUGC provider; this build records the authority
/// boundary and refuses real deletion.
///
/// # Errors
///
/// Returns receipt errors or a provider diagnostic for real deletion.
pub fn delete_workshop_item(
    installation: &InstallationPaths,
    selector: &str,
    dry_run: bool,
    confirmed: bool,
) -> Result<WorkshopOperationReport, String> {
    if !dry_run && !confirmed {
        return Err("real Workshop deletion requires --yes after reviewing --dry-run".to_owned());
    }
    if !dry_run {
        return Err(
            "real Workshop deletion requires the controlled SteamUGC provider; this build can only preview deletion"
                .to_owned(),
        );
    }
    let receipt = write_receipt(
        installation,
        "workshop-delete",
        selector,
        "dry-run",
        Some("would delete or retire the PublishedFileId through SteamUGC"),
    )?;
    Ok(WorkshopOperationReport {
        artifact_id: selector.to_owned(),
        script: None,
        receipt,
        uploaded: false,
        published_file_id: None,
    })
}

/// One exact artifact currently recorded in the app-owned installation index.
#[derive(Debug, Clone)]
pub struct InstalledArtifactSnapshot {
    artifact_id: String,
    name: Option<String>,
    version: Option<String>,
    kind: String,
    workshop_id: Option<String>,
    installed_root: PathBuf,
    fingerprint: Fingerprint,
    source: String,
    enabled: bool,
    dependencies: Vec<ContentReference>,
}

impl InstalledArtifactSnapshot {
    /// Installed content ID.
    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }

    /// Human-readable name authored by the content publisher, when present.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Installed exact version, when declared.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Rust-defined Vapor content kind.
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Workshop PublishedFileId associated with the acquired or installed item.
    pub fn workshop_id(&self) -> Option<&str> {
        self.workshop_id.as_deref()
    }

    /// App-owned installed content directory.
    pub fn installed_root(&self) -> &Path {
        &self.installed_root
    }

    /// Fingerprint recorded for the installed tree.
    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// Acquisition/install source recorded by Vapor (for example `cache` or
    /// `source-package`).
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Whether this record is currently enabled.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Declared dependency and conflict edges from the installed manifest.
    pub fn dependencies(&self) -> &[ContentReference] {
        &self.dependencies
    }
}

/// Check whether an exact installed selector is present in app-owned state.
///
/// # Errors
///
/// Returns filesystem or TOML errors while reading the content index.
pub fn has_installed_artifact(
    installation: &InstallationPaths,
    selector: &str,
) -> Result<bool, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    Ok(resolve_installed_selector(&index, selector).is_ok())
}

/// Read one deployed artifact from Vapor's app-owned cache.
pub fn cached_artifact(
    installation: &InstallationPaths,
    selector: &str,
) -> Result<InstalledArtifactSnapshot, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    let (artifact_id, cached) = index
        .cache_by_selector(selector)
        .ok_or_else(|| format!("content is not in the app-owned cache: {selector}"))?;
    let artifact = read_deployed_manifest(&cached.cache_root)?;
    if artifact.id() != artifact_id {
        return Err(format!(
            "cached manifest identity {} does not match cache index {artifact_id}",
            artifact.id()
        ));
    }
    Ok(InstalledArtifactSnapshot {
        artifact_id,
        name: Some(artifact.name().to_owned()),
        version: artifact.version().map(ToOwned::to_owned),
        kind: artifact.kind().to_string(),
        workshop_id: cached.workshop_id.clone(),
        installed_root: cached.cache_root.clone(),
        fingerprint: cached.fingerprint.clone(),
        source: "cache".to_owned(),
        enabled: true,
        dependencies: artifact.dependencies().to_vec(),
    })
}

/// Read exact installed artifact records from the app-owned index.
///
/// Selectors may be content IDs or exact `id@version` identities. Every selector
/// must resolve; a missing graph node is returned as an error.
///
/// # Errors
///
/// Returns filesystem, TOML, or missing-artifact errors.
pub fn installed_artifacts(
    installation: &InstallationPaths,
    selectors: &[String],
) -> Result<Vec<InstalledArtifactSnapshot>, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    selectors
        .iter()
        .map(|selector| {
            let key = resolve_installed_selector(&index, selector)?;
            let record = index
                .installed
                .get(&key)
                .ok_or_else(|| format!("content is not installed: {selector}"))?;
            let artifact = read_deployed_manifest(&record.installed_root)?;
            Ok(InstalledArtifactSnapshot {
                artifact_id: record.artifact_id.clone(),
                name: Some(artifact.name().to_owned()),
                version: record.version.clone(),
                kind: record.kind.clone(),
                workshop_id: record.workshop_id.clone(),
                installed_root: record.installed_root.clone(),
                fingerprint: record.fingerprint.clone(),
                source: record.source.clone(),
                enabled: record.enabled,
                dependencies: record.dependencies.clone(),
            })
        })
        .collect()
}

fn installed_snapshot_from_record(record: &InstalledRecord) -> InstalledArtifactSnapshot {
    InstalledArtifactSnapshot {
        artifact_id: record.artifact_id.clone(),
        name: None,
        version: record.version.clone(),
        kind: record.kind.clone(),
        workshop_id: record.workshop_id.clone(),
        installed_root: record.installed_root.clone(),
        fingerprint: record.fingerprint.clone(),
        source: record.source.clone(),
        enabled: record.enabled,
        dependencies: record.dependencies.clone(),
    }
}

/// Read the app-owned content index.
///
/// # Errors
///
/// Returns filesystem or TOML parse errors.
pub fn installed_index(installation: &InstallationPaths) -> Result<Vec<String>, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    Ok(index.installed.keys().cloned().collect())
}

/// Read the app-owned cache's content identities for catalog discovery.
///
/// # Errors
///
/// Returns filesystem or TOML errors while reading the content index.
pub fn cached_index(installation: &InstallationPaths) -> Result<Vec<String>, String> {
    let layout = ContentLayout::new(installation);
    let index = load_index(&layout)?;
    Ok(index.caches.keys().cloned().collect())
}

/// Resolve a composition without packaging, downloading, installing, or writing state.
///
/// The selection order follows the shared install behavior: workspace source is
/// preferred for authored roots, compatible installed dependencies are reused,
/// then compatible workspace/cache content is selected. First-party seeds that
/// have not been acquired are reported as blockers because their dependency
/// manifests are not yet locally inspectable.
///
/// # Errors
///
/// Returns source, app-owned index, or cached-manifest inspection errors.
pub fn preview_install(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
) -> Result<InstallPlan, String> {
    let catalog = paths.map(discover).transpose()?;
    let layout = ContentLayout::new(installation);
    let mut provider = LocalInstallProvider {
        installation,
        catalog: catalog.as_ref(),
        index: load_index(&layout)?,
        records: BTreeMap::new(),
    };
    vapor_client_domain::resolve_install_plan(selector, &mut provider)
}

struct LocalInstallProvider<'a> {
    installation: &'a InstallationPaths,
    catalog: Option<&'a ContentCatalog>,
    index: ContentIndex,
    records: BTreeMap<ContentVersionId, ContentArtifact>,
}

impl LocalInstallProvider<'_> {
    fn retain_artifact(
        &mut self,
        artifact: ContentArtifact,
        source: InstallPlanSource,
        already_installed: bool,
    ) -> Result<InstallCandidateAvailability, String> {
        let candidates = source_candidates(std::slice::from_ref(&artifact))?;
        let Some(candidate) = candidates.iter().next().cloned() else {
            return Ok(InstallCandidateAvailability::Unavailable {
                message: Some(format!(
                    "content '{}' has no valid exact semantic version and cannot join a resolved composition",
                    artifact.id(),
                )),
                warning: false,
                seed: None,
            });
        };
        let identity = candidate.identity.clone();
        let artifact = self
            .records
            .entry(identity.clone())
            .or_insert(artifact)
            .clone();
        let conflicts = artifact
            .conflicts()
            .iter()
            .map(|conflict| conflict.id().to_owned())
            .collect::<Vec<_>>();
        let enabled_conflicts = conflicts
            .iter()
            .filter(|id| {
                self.index
                    .installed
                    .get(id.as_str())
                    .is_some_and(|installed| installed.enabled)
            })
            .cloned()
            .collect();
        Ok(InstallCandidateAvailability::Selected(
            SelectedInstallCandidate {
                candidate,
                plan_node: InstallPlanNode {
                    id: artifact.id().to_owned(),
                    version: artifact.version().map(ToOwned::to_owned),
                    kind: artifact.kind().to_string(),
                    published_file_id: artifact
                        .workshop()
                        .published_file_id()
                        .map(ToOwned::to_owned),
                    source,
                    already_installed,
                    dependencies: artifact
                        .dependencies()
                        .iter()
                        .map(|dependency| InstallPlanDependency {
                            id: dependency.id().to_owned(),
                            relationship: dependency.relationship().to_owned(),
                            version: dependency.version().map(ToOwned::to_owned),
                            optional: dependency.optional(),
                        })
                        .collect(),
                },
                conflicts,
                enabled_conflicts,
            },
        ))
    }
}

impl vapor_client_domain::InstallContentProvider for LocalInstallProvider<'_> {
    fn select_root(
        &mut self,
        selector: &str,
    ) -> Result<InstallCandidateAvailability, String> {
        let installed_key = resolve_installed_selector(&self.index, selector).ok();
        let installed = installed_key
            .as_ref()
            .and_then(|key| self.index.installed.get(key))
            .cloned();

        if let Some(artifact) = self
            .catalog
            .and_then(|catalog| catalog.find(selector))
            .cloned()
        {
            return self.retain_artifact(artifact, InstallPlanSource::Workspace, false);
        }

        if let Some((_, cached)) = self.index.cache_by_selector(selector) {
            let artifact = read_deployed_manifest(&cached.cache_root)?;
            return self.retain_artifact(artifact, InstallPlanSource::Cache, false);
        }

        if let Some(installed) = installed.as_ref()
            && installed.enabled
        {
            let artifact = read_deployed_manifest(&installed.installed_root)?;
            return self.retain_artifact(artifact, InstallPlanSource::Installed, true);
        }

        if let Some(seed) = root_content_seed(self.installation, selector)? {
            return Ok(InstallCandidateAvailability::Unavailable {
                message: Some(format!(
                    "{} is supplied by Workshop item {}, but its manifest must be acquired before its dependencies can be resolved",
                    seed.id, seed.workshop_id,
                )),
                warning: false,
                seed: Some(seed_plan_node(seed)),
            });
        }

        if let Some(installed) = installed.as_ref()
            && !installed.enabled
        {
            return Ok(InstallCandidateAvailability::Unavailable {
                message: Some(format!(
                    "{} is installed but disabled, and no compatible source or cached version is available",
                    installed.artifact_id,
                )),
                warning: false,
                seed: None,
            });
        }

        Ok(InstallCandidateAvailability::Unavailable {
            message: Some(format!(
                "required root '{}' is unavailable in the workspace, cache, or enabled installation",
                selector,
            )),
            warning: false,
            seed: None,
        })
    }

    fn select_dependency(
        &mut self,
        parent: &ContentVersionId,
        parent_source: InstallPlanSource,
        binding: &str,
        requirement: &DependencySpec,
    ) -> Result<InstallCandidateAvailability, String> {
        let Some(parent_artifact) = self.records.get(parent) else {
            return Err(format!("resolved parent '{parent}' has no provider record"));
        };
        let Some((_, index)) = binding.rsplit_once('#') else {
            return Err(format!("dependency binding '{binding}' has no source index"));
        };
        let index = index
            .parse::<usize>()
            .map_err(|error| format!("invalid dependency binding '{binding}': {error}"))?;
        let Some(reference) = parent_artifact.dependencies().get(index).cloned() else {
            return Err(format!("dependency binding '{binding}' is not present in its authored artifact"));
        };
        if reference.id() != requirement.id.as_str() {
            return Err(format!("dependency binding '{binding}' does not match its shared identity"));
        }
        let local_closure = parent_source == InstallPlanSource::Workspace;
        let id = requirement.id.as_str();
        let installed = self.index.installed.get(id).cloned();

        if !local_closure
            && let Some(installed) = installed.as_ref()
            && installed.enabled
            && version_satisfies(installed.version.as_deref(), Some(&requirement.version))
        {
            let artifact = read_deployed_manifest(&installed.installed_root)?;
            return self.retain_artifact(artifact, InstallPlanSource::Installed, true);
        }

        if let Some(artifact) = self
            .catalog
            .and_then(|catalog| catalog.satisfying_version(id, Some(&requirement.version)))
            .cloned()
        {
            return self.retain_artifact(artifact, InstallPlanSource::Workspace, false);
        }

        if local_closure {
            return Ok(InstallCandidateAvailability::Unavailable {
                message: Some(format!(
                    "local content requires every descendant to be authored in this workspace, but '{}' has no compatible local source",
                    id,
                )),
                warning: false,
                seed: None,
            });
        }

        let cache_selector = reference.workshop_id().unwrap_or(id);
        if let Some((_, cached)) = self.index.cache_by_selector(cache_selector) {
            let artifact = read_deployed_manifest(&cached.cache_root)?;
            if artifact.id() != id {
                return Ok(InstallCandidateAvailability::Unavailable {
                    message: Some(format!(
                        "Workshop/cache selector for '{}' resolved to '{}'; dependency identity must match exactly",
                        id,
                        artifact.id(),
                    )),
                    warning: false,
                    seed: None,
                });
            }
            if version_satisfies(artifact.version(), Some(&requirement.version)) {
                let already_installed = self
                    .index
                    .installed
                    .get(artifact.id())
                    .is_some_and(|record| {
                        record.enabled && record.version.as_deref() == artifact.version()
                    });
                return self.retain_artifact(
                    artifact,
                    InstallPlanSource::Cache,
                    already_installed,
                );
            }
        }

        if let Some(installed) = installed {
            if !installed.enabled {
                return Ok(InstallCandidateAvailability::Unavailable {
                    message: Some(format!(
                        "{} is installed but disabled, and no compatible source or cached version is available",
                        installed.artifact_id,
                    )),
                    warning: false,
                    seed: None,
                });
            }
            return Ok(InstallCandidateAvailability::Unavailable {
                message: Some(format!(
                    "{} {} does not satisfy required version {}; no compatible source or cached version is available",
                    installed.artifact_id,
                    installed.version.as_deref().unwrap_or("(unversioned)"),
                    reference.version().unwrap_or("any"),
                )),
                warning: false,
                seed: None,
            });
        }

        let seed_selector = reference.workshop_id().unwrap_or(id);
        let seed = match root_content_seed(self.installation, seed_selector)? {
            Some(seed) => Some(seed),
            None if seed_selector != id => root_content_seed(self.installation, id)?,
            None => None,
        };
        if let Some(seed) = seed {
            if seed.id != id {
                return Ok(InstallCandidateAvailability::Unavailable {
                    message: Some(format!(
                        "Workshop seed {} resolves to '{}', but dependency identity must be '{}'",
                        seed.workshop_id, seed.id, id,
                    )),
                    warning: false,
                    seed: None,
                });
            }
            let message = format!(
                "{} is supplied by a Workshop item, but its manifest must be acquired before its dependencies can be resolved",
                id,
            );
            return Ok(InstallCandidateAvailability::Unavailable {
                message: Some(message),
                warning: false,
                seed: Some(seed_plan_node(seed)),
            });
        }

        Ok(InstallCandidateAvailability::Unavailable {
            message: Some(format!(
                "required {} '{}' at {} is unavailable in the workspace, cache, or enabled installation",
                reference.relationship(),
                id,
                reference.version().unwrap_or("any version"),
            )),
            warning: reference.optional(),
            seed: None,
        })
    }
}

fn seed_plan_node(seed: RootContentSeed) -> InstallPlanNode {
    InstallPlanNode {
        id: seed.id,
        version: None,
        kind: seed.kind,
        published_file_id: Some(seed.workshop_id),
        source: InstallPlanSource::RootSeed,
        already_installed: false,
        dependencies: Vec::new(),
    }
}

struct InstallContext<'a> {
    installation: &'a InstallationPaths,
    paths: Option<&'a EnvironmentPaths>,
    catalog: Option<&'a ContentCatalog>,
    layout: &'a ContentLayout,
    account: Option<&'a str>,
    runtime_targets: &'a [RuntimeTarget],
}

struct InstallSelectorRequest<'a> {
    selector: &'a str,
    expected_id: Option<&'a str>,
    parent_kind: Option<ContentKind>,
    relationship: Option<&'a str>,
    require_workspace: bool,
}

struct InstallState<'a> {
    index: &'a mut ContentIndex,
    visiting: &'a mut BTreeSet<String>,
    reports: &'a mut Vec<InstallReport>,
}

fn version_satisfies(version: Option<&str>, requirement: Option<&semver::VersionReq>) -> bool {
    let Some(requirement) = requirement else {
        return true;
    };
    version
        .and_then(|version| semver::Version::parse(version).ok())
        .is_some_and(|version| requirement.matches(&version))
}

fn install_selector(
    context: &InstallContext<'_>,
    request: InstallSelectorRequest<'_>,
    state: &mut InstallState<'_>,
) -> Result<(), String> {
    let InstallSelectorRequest {
        selector,
        expected_id,
        parent_kind,
        relationship,
        require_workspace,
    } = request;
    if let Some(catalog) = context.catalog
        && let Some(artifact) = catalog.find(selector)
    {
        if expected_id.is_some_and(|expected| artifact.id() != expected) {
            return Err(format!(
                "dependency selector '{selector}' resolved to '{}', expected '{}'",
                artifact.id(),
                expected_id.unwrap_or_default()
            ));
        }
        if let (Some(parent_kind), Some(relationship)) = (parent_kind, relationship) {
            validate_dependency_edge(parent_kind, relationship, artifact)?;
        }
        validate_artifact_dependency_roles(artifact)?;
        if !state.visiting.insert(artifact.id().to_owned()) {
            return Err(format!(
                "content dependency cycle includes {}",
                artifact.id()
            ));
        }
        for dependency in artifact
            .dependencies()
            .iter()
            .filter(|item| !item.optional())
        {
            let requirement = dependency
                .version()
                .map(semver::VersionReq::parse)
                .transpose()
                .map_err(|error| {
                    format!(
                        "{} has invalid version requirement for {}: {error}",
                        artifact.id(),
                        dependency.id()
                    )
                })?;
            if let Some(child) = catalog.satisfying_version(dependency.id(), requirement.as_ref()) {
                let versioned_selector = child
                    .version()
                    .map(|version| format!("{}@{}", child.id(), version))
                    .unwrap_or_else(|| child.id().to_owned());
                if let Some(existing) = state
                    .reports
                    .iter()
                    .find(|report| report.artifact_id() == child.id())
                {
                    if existing.version() == child.version() {
                        validate_dependency_edge(
                            artifact.kind(),
                            dependency.relationship(),
                            child,
                        )?;
                        continue;
                    }
                    return Err(format!(
                        "{} resolves two versions of '{}', but app installation currently stores one version per content ID",
                        artifact.id(),
                        child.id()
                    ));
                }
                install_selector(
                    context,
                    InstallSelectorRequest {
                        selector: &versioned_selector,
                        expected_id: Some(child.id()),
                        parent_kind: Some(artifact.kind()),
                        relationship: Some(dependency.relationship()),
                        require_workspace: true,
                    },
                    state,
                )?;
            } else {
                return Err(format!(
                    "local content '{}' requires every descendant to be authored in the selected workspace, but '{}' has no compatible local source",
                    artifact.id(),
                    dependency.id()
                ));
            }
        }
        for conflict in artifact.conflicts() {
            if state
                .index
                .installed
                .get(conflict.id())
                .is_some_and(|record| record.enabled)
            {
                return Err(format!(
                    "{} conflicts with installed content '{}'",
                    artifact.id(),
                    conflict.id()
                ));
            }
        }
        let package_selector = artifact
            .version()
            .map(|version| format!("{}@{}", artifact.id(), version))
            .unwrap_or_else(|| artifact.id().to_owned());
        let package = package_with_runtime_targets(
            context
                .paths
                .expect("source catalog only exists with source paths"),
            &package_selector,
            false,
            context.runtime_targets.to_vec(),
        )?;
        let report = install_package(
            context.installation,
            context.layout,
            state.index,
            artifact,
            package.root(),
            package.fingerprint().clone(),
            "source-package",
        )?;
        state.reports.push(report);
        state.visiting.remove(artifact.id());
        return Ok(());
    }

    if require_workspace {
        return Err(format!(
            "local content requires every descendant to be authored in the selected workspace, but '{selector}' has no compatible local source"
        ));
    }

    if state.index.cache_by_selector(selector).is_none()
        && let Some(seed) = root_content_seed(context.installation, selector)?
    {
        let acquired = acquire_workshop_item(
            context.installation,
            seed.app_id,
            &seed.workshop_id,
            context.account,
        )?;
        state.index.caches.insert(
            acquired.artifact_id().to_owned(),
            CacheRecord {
                artifact_id: acquired.artifact_id().to_owned(),
                workshop_id: Some(seed.workshop_id),
                version: read_deployed_manifest(acquired.cache_root())?
                    .version()
                    .map(ToOwned::to_owned),
                cache_root: acquired.cache_root().to_path_buf(),
                fingerprint: acquired.fingerprint().clone(),
                acquired_at: now_seconds(),
            },
        );
    }

    let cache = state.index
        .cache_by_selector(selector)
        .map(|(_, cache)| cache.clone())
        .ok_or_else(|| {
            format!(
                "cannot install '{selector}': no matching source artifact, app-owned cache entry, or root content seed exists\nhelp: run `content acquire ARTIFACT`, open a source, or add a [[root.content]] seed"
            )
        })?;
    let pseudo = read_deployed_manifest(&cache.cache_root)?;
    if expected_id.is_some_and(|expected| pseudo.id() != expected) {
        return Err(format!(
            "selector '{selector}' resolved to '{}', expected dependency '{}'",
            pseudo.id(),
            expected_id.unwrap_or_default()
        ));
    }
    if let (Some(parent_kind), Some(relationship)) = (parent_kind, relationship) {
        validate_dependency_edge(parent_kind, relationship, &pseudo)?;
    }
    validate_artifact_dependency_roles(&pseudo)?;
    let pseudo_id = pseudo.id().to_owned();
    if !state.visiting.insert(pseudo_id.clone()) {
        return Err(format!("content dependency cycle includes {}", pseudo.id()));
    }
    let result = (|| {
        for dependency in pseudo.dependencies().iter().filter(|item| !item.optional()) {
            let requirement = dependency
                .version()
                .map(semver::VersionReq::parse)
                .transpose()
                .map_err(|error| {
                    format!(
                        "{} has invalid version requirement for {}: {error}",
                        pseudo.id(),
                        dependency.id()
                    )
                })?;
            if state
                .index
                .installed
                .get(dependency.id())
                .is_some_and(|installed| {
                    installed.enabled
                        && version_satisfies(installed.version.as_deref(), requirement.as_ref())
                })
            {
                let installed = state
                    .index
                    .installed
                    .get(dependency.id())
                    .expect("installed dependency was checked above");
                let installed_artifact = read_deployed_manifest(&installed.installed_root)?;
                validate_dependency_edge(
                    pseudo.kind(),
                    dependency.relationship(),
                    &installed_artifact,
                )?;
                continue;
            }
            if let Some(catalog) = context.catalog
                && let Some(child) =
                    catalog.satisfying_version(dependency.id(), requirement.as_ref())
            {
                let selector = child
                    .version()
                    .map(|version| format!("{}@{}", child.id(), version))
                    .unwrap_or_else(|| child.id().to_owned());
                if let Some(existing) = state
                    .reports
                    .iter()
                    .find(|report| report.artifact_id() == child.id())
                {
                    if existing.version() == child.version() {
                        validate_dependency_edge(pseudo.kind(), dependency.relationship(), child)?;
                        continue;
                    }
                    return Err(format!(
                        "{} resolves two versions of '{}', but app installation currently stores one version per content ID",
                        pseudo.id(),
                        child.id()
                    ));
                }
                install_selector(
                    context,
                    InstallSelectorRequest {
                        selector: &selector,
                        expected_id: Some(child.id()),
                        parent_kind: Some(pseudo.kind()),
                        relationship: Some(dependency.relationship()),
                        require_workspace: false,
                    },
                    state,
                )?;
                continue;
            }
            let cache_selector = dependency.workshop_id().unwrap_or_else(|| dependency.id());
            if let Some((cache_id, cached)) = state.index.cache_by_selector(cache_selector) {
                let cached_artifact = read_deployed_manifest(&cached.cache_root)?;
                if version_satisfies(cached_artifact.version(), requirement.as_ref()) {
                    if cached_artifact.id() != dependency.id() {
                        return Err(format!(
                            "Workshop/cache selector for '{}' resolved to '{}'; dependency identity must match exactly",
                            dependency.id(),
                            cached_artifact.id()
                        ));
                    }
                    let selector = dependency.workshop_id().unwrap_or(&cache_id);
                    install_selector(
                        context,
                        InstallSelectorRequest {
                            selector,
                            expected_id: Some(dependency.id()),
                            parent_kind: Some(pseudo.kind()),
                            relationship: Some(dependency.relationship()),
                            require_workspace: false,
                        },
                        state,
                    )?;
                    continue;
                }
            }
            return Err(format!(
                "{} requires {} '{}' at {}, but no compatible installed, local, or cached version is available",
                pseudo.id(),
                dependency.relationship(),
                dependency.id(),
                dependency.version().unwrap_or("any version")
            ));
        }
        for conflict in pseudo.conflicts() {
            if state
                .index
                .installed
                .get(conflict.id())
                .is_some_and(|record| record.enabled)
            {
                return Err(format!(
                    "{} conflicts with installed content '{}'",
                    pseudo.id(),
                    conflict.id()
                ));
            }
        }
        let report = install_package(
            context.installation,
            context.layout,
            state.index,
            &pseudo,
            &cache.cache_root,
            cache.fingerprint,
            "cache",
        )?;
        state.reports.push(report);
        Ok(())
    })();
    state.visiting.remove(&pseudo_id);
    result
}

fn install_package(
    installation: &InstallationPaths,
    layout: &ContentLayout,
    index: &mut ContentIndex,
    artifact: &ContentArtifact,
    artifact_root: &Path,
    expected_fingerprint: Fingerprint,
    source: &str,
) -> Result<InstallReport, String> {
    let target = layout.installed().join(artifact.id());
    reset_directory(installation.root(), &target)?;
    fs::create_dir_all(&target).map_err(io("create installed content", &target))?;
    copy_tree(artifact_root, &target, artifact_root, &[])?;
    let fingerprint = fingerprint_tree(&target)?;
    if fingerprint != expected_fingerprint {
        return Err(format!(
            "installed fingerprint mismatch for {}\n  expected: {}\n  observed: {}",
            artifact.id(),
            expected_fingerprint.hash(),
            fingerprint.hash()
        ));
    }
    let record = InstalledRecord {
        artifact_id: artifact.id().to_owned(),
        kind: artifact.kind().to_string(),
        version: artifact.version().map(ToOwned::to_owned),
        workshop_id: artifact
            .workshop()
            .published_file_id()
            .map(ToOwned::to_owned),
        enabled: true,
        installed_root: target.clone(),
        fingerprint: fingerprint.clone(),
        dependencies: artifact.dependencies().to_vec(),
        conflicts: artifact.conflicts().to_vec(),
        installed_at: now_seconds(),
        source: source.to_owned(),
    };
    index
        .installed
        .insert(artifact.id().to_owned(), record.clone());
    write_lock(layout, &record)?;
    let receipt = write_receipt(
        installation,
        "install",
        artifact.id(),
        "installed",
        Some(&format!("root={}", target.display())),
    )?;
    Ok(InstallReport {
        artifact_id: artifact.id().to_owned(),
        version: artifact.version().map(ToOwned::to_owned),
        installed_root: target,
        fingerprint,
        receipt,
    })
}

fn move_enabled_state(
    installation: &InstallationPaths,
    selector: &str,
    enable: bool,
) -> Result<InstallReport, String> {
    let layout = ContentLayout::new(installation);
    let mut index = load_index(&layout)?;
    let artifact_id = resolve_installed_selector(&index, selector)?;
    let record = index
        .installed
        .get_mut(&artifact_id)
        .ok_or_else(|| format!("content is not installed: {artifact_id}"))?;
    let (from_root, to_root) = if enable {
        (
            layout.disabled().join(&artifact_id),
            layout.installed().join(&artifact_id),
        )
    } else {
        (
            layout.installed().join(&artifact_id),
            layout.disabled().join(&artifact_id),
        )
    };
    if !from_root.exists() {
        return Err(format!(
            "cannot {} {}: expected artifact root is missing at {}",
            if enable { "enable" } else { "disable" },
            artifact_id,
            from_root.display()
        ));
    }
    if let Some(parent) = to_root.parent() {
        fs::create_dir_all(parent).map_err(io("create content state parent", parent))?;
    }
    if to_root.exists() {
        remove_directory(installation.root(), &to_root)?;
    }
    fs::rename(&from_root, &to_root).map_err(io("move content artifact root", &from_root))?;
    let fingerprint = fingerprint_tree(&to_root)?;
    record.enabled = enable;
    record.installed_root = to_root.clone();
    record.fingerprint = fingerprint.clone();
    let record_clone = record.clone();
    let version = record.version.clone();
    save_index(&layout, &index)?;
    write_lock(&layout, &record_clone)?;
    let receipt = write_receipt(
        installation,
        if enable { "enable" } else { "disable" },
        &artifact_id,
        if enable { "enabled" } else { "disabled" },
        Some(&format!("root={}", to_root.display())),
    )?;
    Ok(InstallReport {
        artifact_id,
        version,
        installed_root: to_root,
        fingerprint,
        receipt,
    })
}

fn verify_one(index: &ContentIndex, artifact_id: &str) -> VerifyReport {
    let Some(record) = index.installed.get(artifact_id) else {
        return VerifyReport {
            artifact_id: artifact_id.to_owned(),
            ok: false,
            expected: None,
            observed: None,
            detail: "not installed".to_owned(),
        };
    };
    if !record.installed_root.exists() {
        return VerifyReport {
            artifact_id: artifact_id.to_owned(),
            ok: false,
            expected: Some(record.fingerprint.clone()),
            observed: None,
            detail: format!(
                "installed artifact root is missing: {}",
                record.installed_root.display()
            ),
        };
    }
    match fingerprint_tree(&record.installed_root) {
        Ok(observed) if observed == record.fingerprint => VerifyReport {
            artifact_id: artifact_id.to_owned(),
            ok: true,
            expected: Some(record.fingerprint.clone()),
            observed: Some(observed),
            detail: "fingerprint matches".to_owned(),
        },
        Ok(observed) => VerifyReport {
            artifact_id: artifact_id.to_owned(),
            ok: false,
            expected: Some(record.fingerprint.clone()),
            observed: Some(observed),
            detail: "fingerprint mismatch".to_owned(),
        },
        Err(error) => VerifyReport {
            artifact_id: artifact_id.to_owned(),
            ok: false,
            expected: Some(record.fingerprint.clone()),
            observed: None,
            detail: error,
        },
    }
}

fn select_artifacts<'a>(
    catalog: &'a ContentCatalog,
    selector: Option<&str>,
) -> Result<Vec<&'a ContentArtifact>, String> {
    if let Some(selector) = selector {
        catalog.find(selector).map(|artifact| vec![artifact]).ok_or_else(|| {
            format!(
                "unknown content artifact '{selector}'\nhelp: inspect source content with `content list`"
            )
        })
    } else {
        Ok(catalog.artifacts().iter().collect())
    }
}

fn workspace_version(root: &Path) -> Result<Option<semver::Version>, String> {
    let path = root.join(manifest::WORKSPACE_FILE_NAME);
    let source = fs::read_to_string(&path).map_err(io("read source manifest", &path))?;
    #[derive(Deserialize)]
    struct Root {
        workspace: Option<WorkspaceMeta>,
    }
    #[derive(Deserialize)]
    struct WorkspaceMeta {
        version: Option<String>,
    }
    let parsed: Root = toml::from_str(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", path.display()))?;
    parsed
        .workspace
        .and_then(|workspace| workspace.version)
        .map(|version| {
            semver::Version::parse(&version).map_err(|error| {
                format!(
                    "invalid workspace semantic version '{version}' in '{}': {error}",
                    path.display()
                )
            })
        })
        .transpose()
}

fn read_authored_content(path: &Path, kind: ContentKind) -> Result<AuthoredContent, String> {
    let source = fs::read_to_string(path).map_err(io("read content manifest", path))?;
    let parsed = AuthoredContentManifest::parse(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", path.display()))?;
    let authored = parsed.content(kind).ok_or_else(|| {
        format!(
            "content manifest '{}' did not contain expected [{kind}] section",
            path.display()
        )
    })?;
    let content = AuthoredContent {
        version: authored.version.clone(),
        binaries: authored.binaries.clone(),
        libraries: authored.libraries.clone(),
        workshop: authored.workshop.clone(),
        dependencies: authored
            .dependency_edges(kind)
            .iter()
            .map(|edge| ContentReference::from_authored(&edge.reference, &edge.relationship))
            .collect(),
        conflicts: authored.conflicts.iter().map(|reference| {
            ContentReference::from_authored(reference, &reference.relationship)
        }).collect(),
    };
    Ok(content)
}

#[derive(Debug)]
struct AuthoredContent {
    version: AuthoredVersion,
    binaries: Vec<String>,
    libraries: Vec<String>,
    workshop: WorkshopPolicy,
    dependencies: Vec<ContentReference>,
    conflicts: Vec<ContentReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeployedManifest {
    schema: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    engine: Option<DeployedContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    game: Option<DeployedContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    packagepack: Option<DeployedContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enginepack: Option<DeployedContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gamepack: Option<DeployedContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    modpack: Option<DeployedContent>,
    #[serde(rename = "engine-mod", skip_serializing_if = "Option::is_none")]
    engine_mod: Option<DeployedContent>,
    #[serde(rename = "game-mod", skip_serializing_if = "Option::is_none")]
    game_mod: Option<DeployedContent>,
    #[serde(rename = "extension-mod", skip_serializing_if = "Option::is_none")]
    extension_mod: Option<DeployedContent>,
    #[serde(rename = "crate", skip_serializing_if = "Option::is_none")]
    crate_content: Option<DeployedContent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct DeployedContent {
    id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    binaries: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    libraries: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    runtime: Vec<RuntimePayload>,
    #[serde(
        default,
        rename = "steam",
        skip_serializing_if = "WorkshopPolicy::is_empty"
    )]
    workshop: WorkshopPolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    dependencies: Vec<ContentReference>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    conflicts: Vec<ContentReference>,
}

impl DeployedManifest {
    fn from_artifact(artifact: &ContentArtifact, runtime: Vec<RuntimePayload>) -> Self {
        let content = DeployedContent {
            id: artifact.id().to_owned(),
            name: artifact.name().to_owned(),
            version: artifact.version().map(ToOwned::to_owned),
            binaries: artifact.binaries().to_vec(),
            libraries: artifact.libraries().to_vec(),
            runtime,
            workshop: artifact.workshop().clone(),
            dependencies: artifact.dependencies().to_vec(),
            conflicts: artifact.conflicts().to_vec(),
        };
        let mut manifest = Self::empty();
        match artifact.kind() {
            ContentKind::Engine => manifest.engine = Some(content),
            ContentKind::Game => manifest.game = Some(content),
            ContentKind::Packagepack => manifest.packagepack = Some(content),
            ContentKind::Enginepack => manifest.enginepack = Some(content),
            ContentKind::Gamepack => manifest.gamepack = Some(content),
            ContentKind::Modpack => manifest.modpack = Some(content),
            ContentKind::EngineMod => manifest.engine_mod = Some(content),
            ContentKind::GameMod => manifest.game_mod = Some(content),
            ContentKind::ExtensionMod => manifest.extension_mod = Some(content),
            ContentKind::Crate => manifest.crate_content = Some(content),
        }
        manifest
    }

    fn empty() -> Self {
        Self {
            schema: CONTENT_STATE_SCHEMA,
            engine: None,
            game: None,
            packagepack: None,
            enginepack: None,
            gamepack: None,
            modpack: None,
            engine_mod: None,
            game_mod: None,
            extension_mod: None,
            crate_content: None,
        }
    }

}

fn stage_deployed_artifact(
    paths: &EnvironmentPaths,
    package_root: &Path,
    artifact: &ContentArtifact,
    runtime_targets: &[RuntimeTarget],
) -> Result<(), String> {
    validate_runtime_output_names("binaries", artifact.binaries())?;
    validate_runtime_output_names("libraries", artifact.libraries())?;
    copy_tree(
        artifact.root(),
        package_root,
        artifact.root(),
        default_exclusions(),
    )?;
    let mut runtime = artifact.runtime().to_vec();
    for runtime_target in runtime_targets {
        let staged_runtime = stage_runtime_outputs(paths, package_root, artifact, runtime_target)?;
        if staged_runtime.is_empty() {
            continue;
        }
        runtime.retain(|payload| payload.target != runtime_target.triple());
        runtime.push(RuntimePayload {
            target: runtime_target.triple().to_owned(),
            binaries: staged_runtime.binaries,
            libraries: staged_runtime.libraries,
        });
    }
    runtime.sort_by(|left, right| left.target.cmp(&right.target));
    write_deployed_manifest(package_root, artifact, runtime)
}

fn stage_runtime_outputs(
    paths: &EnvironmentPaths,
    package_root: &Path,
    artifact: &ContentArtifact,
    runtime_target: &RuntimeTarget,
) -> Result<StagedRuntimeOutputs, String> {
    let mut staged = StagedRuntimeOutputs::default();
    if artifact.binaries().is_empty() && artifact.libraries().is_empty() {
        return Ok(staged);
    }
    let build_root = content_build_output_root(paths, runtime_target)?;
    let bin_root = package_root.join("bin").join(runtime_target.triple());
    let lib_root = package_root.join("lib").join(runtime_target.triple());
    for binary in artifact.binaries() {
        let source = find_built_binary(&build_root, binary, runtime_target.triple())?;
        staged
            .binaries
            .push(copy_runtime_output(&source, &bin_root)?);
    }
    for library in artifact.libraries() {
        let source = find_built_library(&build_root, library, runtime_target.triple())?;
        staged
            .libraries
            .push(copy_runtime_output(&source, &lib_root)?);
    }
    if !staged.binaries.is_empty() {
        cross_toolchain::copy_windows_runtime_dlls(
            paths.installation().root(),
            runtime_target.triple(),
            &bin_root,
        )?;
    }
    if !staged.libraries.is_empty() {
        cross_toolchain::copy_windows_runtime_dlls(
            paths.installation().root(),
            runtime_target.triple(),
            &lib_root,
        )?;
    }
    Ok(staged)
}

#[derive(Debug, Clone)]
struct RuntimeTarget {
    triple: String,
    explicit: bool,
}

impl RuntimeTarget {
    fn new(target_triple: Option<&str>) -> Result<Self, String> {
        match target_triple {
            Some(target) => {
                validate_runtime_target(target)?;
                Ok(Self {
                    triple: target.to_owned(),
                    explicit: true,
                })
            }
            None => Ok(Self::host()),
        }
    }

    fn many(target_triples: &[String]) -> Result<Vec<Self>, String> {
        if target_triples.is_empty() {
            return Ok(vec![Self::host()]);
        }
        let mut seen = BTreeSet::new();
        let mut targets = Vec::new();
        for target in target_triples {
            let runtime_target = Self::new(Some(target))?;
            if !seen.insert(runtime_target.triple.clone()) {
                return Err(format!("duplicate runtime target: {target}"));
            }
            targets.push(runtime_target);
        }
        Ok(targets)
    }

    fn host() -> Self {
        Self {
            triple: host_runtime_target(),
            explicit: false,
        }
    }

    fn triple(&self) -> &str {
        &self.triple
    }
}

fn runtime_target_label(runtime_targets: &[RuntimeTarget]) -> String {
    runtime_targets
        .iter()
        .map(|target| target.triple())
        .collect::<Vec<_>>()
        .join(",")
}

#[derive(Debug, Default)]
struct StagedRuntimeOutputs {
    binaries: Vec<String>,
    libraries: Vec<String>,
}

impl StagedRuntimeOutputs {
    fn is_empty(&self) -> bool {
        self.binaries.is_empty() && self.libraries.is_empty()
    }
}

pub(crate) fn host_runtime_target() -> String {
    let arch = std::env::consts::ARCH;
    match (arch, std::env::consts::OS, std::env::consts::FAMILY) {
        ("x86_64", "linux", _) => "x86_64-unknown-linux-gnu".to_owned(),
        ("aarch64", "linux", _) => "aarch64-unknown-linux-gnu".to_owned(),
        ("x86_64", "windows", _) => "x86_64-pc-windows-gnullvm".to_owned(),
        ("aarch64", "windows", _) => "aarch64-pc-windows-msvc".to_owned(),
        ("x86_64", "macos", _) => "x86_64-apple-darwin".to_owned(),
        ("aarch64", "macos", _) => "aarch64-apple-darwin".to_owned(),
        _ => format!(
            "{arch}-{}-{}",
            std::env::consts::OS,
            std::env::consts::FAMILY
        ),
    }
}

fn validate_runtime_target(target: &str) -> Result<(), String> {
    let valid = !target.is_empty()
        && target
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    if valid {
        Ok(())
    } else {
        Err(format!(
            "runtime target must be a Rust target triple such as x86_64-pc-windows-gnullvm: {target}"
        ))
    }
}

fn binary_suffix(target: &str) -> &'static str {
    if target.contains("windows") {
        ".exe"
    } else {
        ""
    }
}

fn library_file_names(stem: &str, target: &str) -> Vec<String> {
    if target.contains("windows") {
        vec![format!("{stem}.dll"), format!("{stem}.lib")]
    } else if target.contains("darwin") || target.contains("apple") {
        vec![format!("lib{stem}.dylib"), format!("lib{stem}.rlib")]
    } else {
        vec![format!("lib{stem}.so"), format!("lib{stem}.rlib")]
    }
}

fn content_build_output_root(
    paths: &EnvironmentPaths,
    runtime_target: &RuntimeTarget,
) -> Result<PathBuf, String> {
    #[derive(Deserialize)]
    struct RootManifest {
        workspace: Option<WorkspaceSection>,
    }
    #[derive(Deserialize)]
    struct WorkspaceSection {
        name: String,
    }

    let path = paths.source().root().join(manifest::WORKSPACE_FILE_NAME);
    let source = fs::read_to_string(&path).map_err(io("read source manifest", &path))?;
    let parsed: RootManifest = toml::from_str(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", path.display()))?;
    let name = parsed
        .workspace
        .map(|workspace| workspace.name)
        .ok_or_else(|| {
            format!(
                "content runtime outputs require a [workspace] source manifest: {}",
                path.display()
            )
        })?;
    let root = paths.installation().root().join("output/dev").join(name);
    if runtime_target.explicit {
        Ok(root.join(runtime_target.triple()).join("debug"))
    } else {
        Ok(root.join("debug"))
    }
}

fn find_built_binary(build_root: &Path, name: &str, target: &str) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    let suffix = binary_suffix(target);
    if suffix.is_empty() || name.ends_with(suffix) {
        candidates.push(build_root.join(name));
    } else {
        candidates.push(build_root.join(format!("{name}{suffix}")));
        candidates.push(build_root.join(name));
    }
    find_existing_runtime_output("binary", name, candidates)
}

fn find_built_library(build_root: &Path, name: &str, target: &str) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if Path::new(name).extension().is_none() {
        for stem in [name.to_owned(), name.replace('-', "_")] {
            for candidate in library_file_names(&stem, target) {
                candidates.push(build_root.join(candidate));
            }
        }
    } else {
        candidates.push(build_root.join(name));
    }
    let mut deduped = Vec::new();
    for candidate in candidates {
        if !deduped.contains(&candidate) {
            deduped.push(candidate);
        }
    }
    find_existing_runtime_output("library", name, deduped)
}

fn find_existing_runtime_output(
    kind: &str,
    name: &str,
    candidates: Vec<PathBuf>,
) -> Result<PathBuf, String> {
    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }
    let checked = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "declared content {kind} '{name}' was not built; checked: {checked}\nhelp: run `content build` before `content package` or use `content deploy`"
    ))
}

fn copy_runtime_output(source: &Path, target_directory: &Path) -> Result<String, String> {
    fs::create_dir_all(target_directory)
        .map_err(io("create runtime output directory", target_directory))?;
    let file_name = source
        .file_name()
        .ok_or_else(|| format!("runtime output has no filename: {}", source.display()))?;
    let target = target_directory.join(file_name);
    fs::copy(source, &target).map_err(io("copy runtime output", source))?;
    Ok(file_name.to_string_lossy().into_owned())
}

fn validate_runtime_output_names(label: &str, names: &[String]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for name in names {
        let mut components = Path::new(name).components();
        let valid = matches!(
            components.next(),
            Some(Component::Normal(part)) if part.to_str() == Some(name.as_str())
        ) && components.next().is_none();
        if !valid {
            return Err(format!(
                "content {label} must be file names, not paths: {name}"
            ));
        }
        if !seen.insert(name) {
            return Err(format!("duplicate content {label} entry: {name}"));
        }
    }
    Ok(())
}

fn write_deployed_manifest(
    package_root: &Path,
    artifact: &ContentArtifact,
    runtime: Vec<RuntimePayload>,
) -> Result<(), String> {
    let manifest = DeployedManifest::from_artifact(artifact, runtime);
    let encoded = toml::to_string_pretty(&manifest)
        .map_err(|error| format!("failed to encode deployed content manifest: {error}"))?;
    let path = package_root.join(artifact.kind().manifest_file_name());
    fs::write(&path, encoded).map_err(io("write deployed content manifest", &path))
}

fn read_deployed_manifest(package_root: &Path) -> Result<ContentArtifact, String> {
    let path = manifest::content_marker_at(package_root).ok_or_else(|| {
        format!(
            "deployed content root '{}' has no role-specific content manifest",
            package_root.display()
        )
    })?;
    let kind = ContentKind::ALL
        .into_iter()
        .find(|kind| {
            path.file_name()
                .and_then(|name| name.to_str())
                == Some(kind.manifest_file_name())
        })
        .ok_or_else(|| {
            format!(
                "unrecognized deployed content manifest '{}': role is unknown",
                path.display()
            )
        })?;
    let source = fs::read_to_string(&path).map_err(io("read deployed content manifest", &path))?;
    let manifest = AuthoredContentManifest::parse(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", path.display()))?;
    match manifest.schema_version() {
        Some(schema) if schema > 0 => {}
        _ => {
            return Err(format!(
                "deployed content manifest '{}' has no valid schema version",
                path.display()
            ));
        }
    }
    if manifest.content_count() != 1 {
        return Err(format!(
            "deployed content manifest '{}' must contain exactly one role section",
            path.display()
        ));
    }
    let authored = manifest
        .content(kind)
        .ok_or_else(|| {
            format!(
                "deployed content manifest '{}' does not match its filename role",
                path.display()
            )
        })?;
    let id = authored
        .id
        .as_ref()
        .ok_or_else(|| {
            format!(
                "deployed content manifest '{}' has no content ID",
                path.display()
            )
        })?
        .to_string();
    let name = authored
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "deployed content manifest '{}' has no content name",
                path.display()
            )
        })?;
    let version = match &authored.version {
        AuthoredVersion::Exact(version) => Some(version.to_string()),
        AuthoredVersion::Missing | AuthoredVersion::Workspace { workspace: false } => None,
        AuthoredVersion::Workspace { workspace: true } => {
            return Err(format!(
                "deployed content manifest '{}' cannot inherit a workspace version",
                path.display()
            ));
        }
    };
    let dependencies = authored
        .dependency_edges(kind)
        .iter()
        .map(|edge| ContentReference::from_authored(&edge.reference, &edge.relationship))
        .collect();
    let conflicts = authored
        .conflicts
        .iter()
        .map(|reference| ContentReference::from_authored(reference, &reference.relationship))
        .collect();
    Ok(ContentArtifact {
        id,
        name,
        kind,
        root: package_root.to_path_buf(),
        manifest: path,
        version,
        binaries: authored.binaries.clone(),
        libraries: authored.libraries.clone(),
        runtime: authored
            .runtime
            .iter()
            .map(|payload| RuntimePayload {
                target: payload.target.clone(),
                binaries: payload.binaries.clone(),
                libraries: payload.libraries.clone(),
            })
            .collect(),
        dependencies,
        conflicts,
        workshop: authored.workshop.clone(),
    })
}

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new(prefix: &str, artifact_id: &str) -> Result<Self, String> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let base = std::env::temp_dir();
        for attempt in 0..16 {
            let path = base.join(format!(
                "{}-{}-{}-{}-{}",
                prefix,
                slug(artifact_id),
                std::process::id(),
                stamp,
                attempt
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!(
                        "failed to create temporary directory '{}': {error}",
                        path.display()
                    ));
                }
            }
        }
        Err(format!(
            "failed to create temporary directory for {} after repeated attempts",
            artifact_id
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstalledRecord {
    artifact_id: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workshop_id: Option<String>,
    enabled: bool,
    installed_root: PathBuf,
    fingerprint: Fingerprint,
    #[serde(default)]
    dependencies: Vec<ContentReference>,
    #[serde(default)]
    conflicts: Vec<ContentReference>,
    installed_at: u64,
    source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheRecord {
    artifact_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    workshop_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    cache_root: PathBuf,
    fingerprint: Fingerprint,
    acquired_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ContentIndex {
    schema_version: u32,
    #[serde(default)]
    installed: BTreeMap<String, InstalledRecord>,
    #[serde(default)]
    caches: BTreeMap<String, CacheRecord>,
}

impl Default for ContentIndex {
    fn default() -> Self {
        Self {
            schema_version: CONTENT_STATE_SCHEMA,
            installed: BTreeMap::new(),
            caches: BTreeMap::new(),
        }
    }
}

impl ContentIndex {
    fn cache_by_selector(&self, selector: &str) -> Option<(String, &CacheRecord)> {
        let (identity, requested_version) = selector
            .rsplit_once('@')
            .map(|(identity, version)| (identity, Some(version)))
            .unwrap_or((selector, None));
        self.caches.iter().find_map(|(id, cache)| {
            ((id == identity
                || cache.artifact_id == identity
                || cache
                    .workshop_id
                    .as_deref()
                    .is_some_and(|workshop_id| workshop_id == identity))
                && requested_version
                    .is_none_or(|version| cache.version.as_deref() == Some(version)))
            .then(|| (id.clone(), cache))
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OperationReceipt {
    schema_version: u32,
    action: String,
    artifact_id: String,
    status: String,
    timestamp: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

fn load_index(layout: &ContentLayout) -> Result<ContentIndex, String> {
    if !layout.index().is_file() {
        return Ok(ContentIndex::default());
    }
    let source =
        fs::read_to_string(layout.index()).map_err(io("read content index", layout.index()))?;
    let mut index: ContentIndex = toml::from_str(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", layout.index().display()))?;
    if index.schema_version == 0 {
        index.schema_version = CONTENT_STATE_SCHEMA;
    }
    Ok(index)
}

fn save_index(layout: &ContentLayout, index: &ContentIndex) -> Result<(), String> {
    if let Some(parent) = layout.index().parent() {
        fs::create_dir_all(parent).map_err(io("create content index parent", parent))?;
    }
    let encoded = toml::to_string_pretty(index)
        .map_err(|error| format!("failed to encode content index: {error}"))?;
    fs::write(layout.index(), encoded).map_err(io("write content index", layout.index()))
}

fn write_lock(layout: &ContentLayout, record: &InstalledRecord) -> Result<(), String> {
    fs::create_dir_all(layout.locks()).map_err(io("create content locks", layout.locks()))?;
    let path = layout
        .locks()
        .join(format!("{}.toml", slug(&record.artifact_id)));
    let encoded = toml::to_string_pretty(record)
        .map_err(|error| format!("failed to encode content lock: {error}"))?;
    fs::write(&path, encoded).map_err(io("write content lock", &path))
}

fn write_receipt(
    installation: &InstallationPaths,
    action: &str,
    artifact_id: &str,
    status: &str,
    detail: Option<&str>,
) -> Result<PathBuf, String> {
    let layout = ContentLayout::new(installation);
    fs::create_dir_all(layout.receipts())
        .map_err(io("create content receipts", layout.receipts()))?;
    let receipt = OperationReceipt {
        schema_version: CONTENT_STATE_SCHEMA,
        action: action.to_owned(),
        artifact_id: artifact_id.to_owned(),
        status: status.to_owned(),
        timestamp: now_seconds(),
        detail: detail.map(ToOwned::to_owned),
    };
    let path = layout.receipts().join(format!(
        "{}-{}-{}.toml",
        receipt.timestamp,
        action,
        slug(artifact_id)
    ));
    let encoded = toml::to_string_pretty(&receipt)
        .map_err(|error| format!("failed to encode content receipt: {error}"))?;
    fs::write(&path, encoded).map_err(io("write content receipt", &path))?;
    Ok(path)
}

fn resolve_installed_selector(index: &ContentIndex, selector: &str) -> Result<String, String> {
    if let Some((id, version)) = selector.rsplit_once('@')
        && !id.is_empty()
        && !version.is_empty()
    {
        return index
            .installed
            .iter()
            .find_map(|(key, record)| {
                (record.artifact_id == id && record.version.as_deref() == Some(version))
                    .then(|| key.clone())
            })
            .ok_or_else(|| format!("content version is not installed: {selector}"));
    }
    index
        .installed
        .iter()
        .find_map(|(id, record)| {
            (id == selector
                || record.artifact_id == selector
                || record
                    .workshop_id
                    .as_deref()
                    .is_some_and(|workshop_id| workshop_id == selector))
            .then(|| id.clone())
        })
        .ok_or_else(|| format!("content is not installed: {selector}"))
}

fn write_workshop_script(
    paths: &EnvironmentPaths,
    artifact: &ContentArtifact,
    artifact_root: &Path,
    change_note: Option<&str>,
    preview: bool,
) -> Result<PathBuf, String> {
    let layout = ContentLayout::new(paths.installation());
    fs::create_dir_all(layout.output_scripts())
        .map_err(io("create Workshop script output", layout.output_scripts()))?;
    let script = layout
        .output_scripts()
        .join(format!("workshop_{}.vdf", slug(artifact.id())));
    let policy = artifact.workshop();
    let preview_line = if preview {
        "    \"preview\" \"1\"\n"
    } else {
        ""
    };
    let mut tags = String::new();
    tags.push_str("    \"tags\"\n    {\n");
    let mut all_tags = Vec::new();
    for tag in std::iter::once(artifact.kind().to_string()).chain(policy.tags().iter().cloned()) {
        if !all_tags.contains(&tag) {
            all_tags.push(tag);
        }
    }
    for (index, tag) in all_tags.iter().enumerate() {
        tags.push_str(&format!(
            "        \"{}\" \"{}\"\n",
            index,
            steam_escape(tag)
        ));
    }
    tags.push_str("    }\n");
    let vdf = format!(
        "\"workshopitem\"\n{{\n    \"appid\" \"{}\"\n{}    \"publishedfileid\" \"{}\"\n    \"contentfolder\" \"{}\"\n    \"visibility\" \"{}\"\n    \"title\" \"{}\"\n    \"description\" \"{}\"\n    \"changenote\" \"{}\"\n{tags}}}\n",
        policy.app_id().expect("app id checked"),
        preview_line,
        steam_escape(policy.published_file_id().unwrap_or("0")),
        steam_escape(&artifact_root.display().to_string()),
        steam_escape(policy.visibility().unwrap_or("private")),
        steam_escape(policy.title().unwrap_or(artifact.name())),
        steam_escape(policy.description().unwrap_or("")),
        steam_escape(
            change_note
                .or_else(|| policy.change_note())
                .unwrap_or("Vapor content update")
        ),
    );
    fs::write(&script, vdf).map_err(io("write Workshop script", &script))?;
    Ok(script)
}

fn run_steamcmd_workshop_build(
    paths: &EnvironmentPaths,
    account: &str,
    script: &Path,
) -> Result<(), String> {
    run_steamcmd_workshop_builds(paths, account, &[script.to_path_buf()])
}

fn run_steamcmd_workshop_builds(
    paths: &EnvironmentPaths,
    account: &str,
    scripts: &[PathBuf],
) -> Result<(), String> {
    if scripts.is_empty() {
        return Err("at least one Workshop build script is required".to_owned());
    }
    let steamcmd = steam::executable(paths)?;
    let mut command = Command::new(&steamcmd);
    command.args(["+login", account]);
    for script in scripts {
        command.arg("+workshop_build_item").arg(script);
    }
    let status = command
        .arg("+quit")
        .current_dir(steamcmd.parent().expect("SteamCMD has a parent"))
        .status()
        .map_err(|error| format!("failed to start SteamCMD: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Steam Workshop build exited with {status}"))
    }
}

fn run_steamcmd_workshop_download(
    installation: &InstallationPaths,
    app_id: u32,
    published_file_id: &str,
    account: &str,
) -> Result<PathBuf, String> {
    let mut roots = run_steamcmd_workshop_downloads(
        installation,
        &[WorkshopDownload {
            app_id,
            published_file_id: published_file_id.to_owned(),
        }],
        account,
    )?;
    roots
        .pop()
        .ok_or_else(|| "SteamCMD returned no Workshop download root".to_owned())
}

#[derive(Debug, Clone)]
struct WorkshopDownload {
    app_id: u32,
    published_file_id: String,
}

fn run_steamcmd_workshop_downloads(
    installation: &InstallationPaths,
    downloads: &[WorkshopDownload],
    account: &str,
) -> Result<Vec<PathBuf>, String> {
    if downloads.is_empty() {
        return Err("at least one Workshop download is required".to_owned());
    }
    let steamcmd = steam::executable_for_installation(installation)?;
    let mut command = Command::new(&steamcmd);
    command.args(["+login", account]);
    for download in downloads {
        command
            .arg("+workshop_download_item")
            .arg(download.app_id.to_string())
            .arg(&download.published_file_id);
    }
    let status = command
        .arg("+quit")
        .current_dir(steamcmd.parent().expect("SteamCMD has a parent"))
        .status()
        .map_err(|error| format!("failed to start SteamCMD: {error}"))?;
    if !status.success() {
        return Err(format!("Steam Workshop download exited with {status}"));
    }
    downloads
        .iter()
        .map(|download| {
            resolve_steamcmd_workshop_download_root(
                installation,
                &steamcmd,
                download.app_id,
                &download.published_file_id,
            )
        })
        .collect()
}

fn resolve_steamcmd_workshop_download_root(
    installation: &InstallationPaths,
    steamcmd: &Path,
    app_id: u32,
    published_file_id: &str,
) -> Result<PathBuf, String> {
    let app_id = app_id.to_string();
    let mut candidates = Vec::new();
    if let Some(parent) = steamcmd.parent() {
        candidates.push(
            parent
                .join("steamapps/workshop/content")
                .join(&app_id)
                .join(published_file_id),
        );
    }
    if let Some(common_dir) = installation.root().parent()
        && common_dir
            .file_name()
            .is_some_and(|name| name.to_string_lossy() == "common")
        && let Some(steamapps_dir) = common_dir.parent()
    {
        candidates.push(
            steamapps_dir
                .join("workshop/content")
                .join(&app_id)
                .join(published_file_id),
        );
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.push(
            home.join(".local/share/Steam/steamapps/workshop/content")
                .join(&app_id)
                .join(published_file_id),
        );
        candidates.push(
            home.join(".steam/steam/steamapps/workshop/content")
                .join(&app_id)
                .join(published_file_id),
        );
    }
    for download_root in &candidates {
        if download_root.exists() {
            return Ok(download_root.clone());
        }
    }
    let attempted = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "SteamCMD reported success but no Workshop content folder exists; checked: {attempted}"
    ))
}

fn read_workshop_script_published_file_id(script: &Path) -> Result<String, String> {
    let source = fs::read_to_string(script).map_err(io("read Workshop script", script))?;
    vdf_value(&source, "publishedfileid")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "Workshop script has no publishedfileid: {}",
                script.display()
            )
        })
}

fn record_published_file_id(
    artifact: &ContentArtifact,
    published_file_id: &str,
) -> Result<(), String> {
    let source = fs::read_to_string(artifact.manifest())
        .map_err(io("read content manifest", artifact.manifest()))?;
    let updated = set_published_file_id(&source, artifact.kind(), published_file_id)?;
    AuthoredContentManifest::parse(&updated).map_err(|error| {
        format!(
            "refusing to write invalid content manifest '{}': {error}",
            artifact.manifest().display()
        )
    })?;
    fs::write(artifact.manifest(), updated)
        .map_err(io("write content manifest", artifact.manifest()))
}

fn set_published_file_id(
    source: &str,
    kind: ContentKind,
    published_file_id: &str,
) -> Result<String, String> {
    let section = format!("[{}.steam]", kind);
    let mut lines = source.lines().map(ToOwned::to_owned).collect::<Vec<_>>();
    let Some(start) = lines.iter().position(|line| line.trim() == section) else {
        return Err(format!("content manifest has no {section} section"));
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find_map(|(index, line)| line.trim_start().starts_with('[').then_some(index))
        .unwrap_or(lines.len());
    let field = format!("published-file-id = \"{}\"", published_file_id);
    if let Some(index) = lines[start + 1..end].iter().position(|line| {
        line.trim_start()
            .strip_prefix("published-file-id")
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    }) {
        lines[start + 1 + index] = field;
    } else {
        let insert_at = lines[start + 1..end]
            .iter()
            .position(|line| {
                line.trim_start()
                    .strip_prefix("app-id")
                    .is_some_and(|rest| rest.trim_start().starts_with('='))
            })
            .map_or(start + 1, |index| start + 2 + index);
        lines.insert(insert_at, field);
    }
    let mut updated = lines.join("\n");
    if source.ends_with('\n') {
        updated.push('\n');
    }
    Ok(updated)
}

fn vdf_value(source: &str, key: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let parts = line.trim().split('"').collect::<Vec<_>>();
        (parts.len() >= 4 && parts[1] == key).then(|| parts[3].to_owned())
    })
}

fn is_published_file_id(selector: &str) -> bool {
    !selector.is_empty() && selector.bytes().all(|byte| byte.is_ascii_digit())
}

fn resolve_download_app_id(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    selector: &str,
) -> Result<u32, String> {
    if let Some(paths) = paths {
        let catalog = discover(paths)?;
        if let Some(artifact) = catalog.find(selector)
            && let Some(app_id) = artifact.workshop().app_id()
        {
            return Ok(app_id);
        }
    }
    root_steam_app_id(installation)
}

/// Resolve the Steam application that owns a Workshop item in an install plan.
///
/// First-party seeds carry their own application ID. Other items use the
/// discovered source metadata or the app's Workshop application.
pub fn workshop_app_id(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    artifact_id: &str,
    published_file_id: &str,
) -> Result<u32, String> {
    if let Some(seed) = root_content_seed(installation, artifact_id)?
        .or(root_content_seed(installation, published_file_id)?)
    {
        return Ok(seed.app_id);
    }
    resolve_download_app_id(installation, paths, published_file_id)
}

fn resolve_live_workshop_download(
    installation: &InstallationPaths,
    paths: Option<&EnvironmentPaths>,
    index: &ContentIndex,
    selector: &str,
) -> Result<Option<WorkshopDownload>, String> {
    if is_published_file_id(selector) {
        return Ok(Some(WorkshopDownload {
            app_id: resolve_download_app_id(installation, paths, selector)?,
            published_file_id: selector.to_owned(),
        }));
    }

    if let Some(paths) = paths {
        let catalog = discover(paths)?;
        if let Some(artifact) = catalog.find(selector)
            && let (Some(app_id), Some(published_file_id)) = (
                artifact.workshop().app_id(),
                artifact.workshop().published_file_id(),
            )
        {
            return Ok(Some(WorkshopDownload {
                app_id,
                published_file_id: published_file_id.to_owned(),
            }));
        }
    }

    if let Some(seed) = root_content_seed(installation, selector)? {
        return Ok(Some(WorkshopDownload {
            app_id: seed.app_id,
            published_file_id: seed.workshop_id,
        }));
    }

    if let Some((_artifact_id, cache)) = index.cache_by_selector(selector)
        && let Some(published_file_id) = &cache.workshop_id
    {
        let artifact = read_deployed_manifest(&cache.cache_root)?;
        let app_id = artifact
            .workshop()
            .app_id()
            .map_or_else(|| root_steam_app_id(installation), Ok)?;
        return Ok(Some(WorkshopDownload {
            app_id,
            published_file_id: published_file_id.clone(),
        }));
    }

    Ok(None)
}

fn root_steam_app_id(installation: &InstallationPaths) -> Result<u32, String> {
    #[derive(Deserialize)]
    struct RootManifest {
        root: Option<RootSection>,
    }
    #[derive(Deserialize)]
    struct RootSection {
        steam: Option<RootSteam>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct RootSteam {
        app_id: Option<u32>,
    }

    let path = installation.root().join(manifest::APP_FILE_NAME);
    let source = fs::read_to_string(&path).map_err(io("read root manifest", &path))?;
    let manifest: RootManifest = toml::from_str(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", path.display()))?;
    manifest
        .root
        .and_then(|root| root.steam)
        .and_then(|steam| steam.app_id)
        .filter(|app_id| *app_id != 0)
        .ok_or_else(|| {
            format!(
                "cannot infer Steam AppID for Workshop download; '{}' needs [root.steam].app-id",
                path.display()
            )
        })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct RootContentSeed {
    id: String,
    kind: String,
    app_id: u32,
    workshop_id: String,
    default_launch: Option<String>,
}

fn root_content_seed(
    installation: &InstallationPaths,
    selector: &str,
) -> Result<Option<RootContentSeed>, String> {
    #[derive(Deserialize)]
    struct RootManifest {
        root: Option<RootSection>,
    }
    #[derive(Deserialize)]
    struct RootSection {
        #[serde(default)]
        content: Vec<RootContentSeed>,
    }

    let path = installation.root().join(manifest::APP_FILE_NAME);
    let source = fs::read_to_string(&path).map_err(io("read root manifest", &path))?;
    let manifest: RootManifest = toml::from_str(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", path.display()))?;
    Ok(manifest
        .root
        .into_iter()
        .flat_map(|root| root.content)
        .find(|seed| root_content_seed_matches(seed, selector)))
}

fn root_content_seed_matches(seed: &RootContentSeed, selector: &str) -> bool {
    seed.id == selector
        || seed
            .id
            .rsplit('/')
            .next()
            .is_some_and(|name| name == selector)
        || seed.workshop_id == selector
        || seed
            .default_launch
            .as_deref()
            .is_some_and(|launch| launch == selector)
}

fn fingerprint_tree(root: &Path) -> Result<Fingerprint, String> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    let mut hash = 0xcbf29ce484222325u64;
    let mut file_count = 0u64;
    let mut byte_count = 0u64;
    for relative in files {
        let path = root.join(&relative);
        update_hash(&mut hash, relative.to_string_lossy().as_bytes());
        update_hash(&mut hash, &[0]);
        let mut file = fs::File::open(&path).map_err(io("open fingerprint input", &path))?;
        let mut buffer = [0u8; 8192];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(io("read fingerprint input", &path))?;
            if read == 0 {
                break;
            }
            update_hash(&mut hash, &buffer[..read]);
            byte_count += read as u64;
        }
        file_count += 1;
    }
    Ok(Fingerprint {
        algorithm: FINGERPRINT_ALGORITHM.to_owned(),
        hash: format!("{hash:016x}"),
        files: file_count,
        bytes: byte_count,
    })
}

fn collect_files(root: &Path, directory: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(io("read fingerprint directory", directory))? {
        let entry = entry.map_err(|error| format!("failed to read fingerprint entry: {error}"))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect '{}': {error}", path.display()))?;
        if file_type.is_dir() {
            collect_files(root, &path, output)?;
        } else if file_type.is_file() {
            output.push(
                path.strip_prefix(root)
                    .map_err(|error| format!("failed to relativize '{}': {error}", path.display()))?
                    .to_path_buf(),
            );
        }
    }
    Ok(())
}

fn update_hash(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

fn copy_tree(source: &Path, target: &Path, root: &Path, exclusions: &[&str]) -> Result<(), String> {
    let relative = source.strip_prefix(root).unwrap_or(Path::new(""));
    if exclusions
        .iter()
        .any(|excluded| relative.starts_with(Path::new(excluded)))
    {
        return Ok(());
    }
    let metadata = fs::metadata(source).map_err(io("inspect copy source", source))?;
    if metadata.is_dir() {
        fs::create_dir_all(target).map_err(io("create copy target", target))?;
        for entry in fs::read_dir(source).map_err(io("read copy source", source))? {
            let entry = entry.map_err(|error| format!("failed to read copy entry: {error}"))?;
            copy_tree(
                &entry.path(),
                &target.join(entry.file_name()),
                root,
                exclusions,
            )?;
        }
    } else if metadata.is_file() {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io("create copy target parent", parent))?;
        }
        fs::copy(source, target).map_err(io("copy content file", source))?;
    }
    Ok(())
}

fn default_exclusions() -> &'static [&'static str] {
    &[".git", "target", "output"]
}

fn reset_directory(app_root: &Path, target: &Path) -> Result<(), String> {
    ensure_contained(app_root, target)?;
    if target.exists() {
        remove_directory(app_root, target)?;
    }
    fs::create_dir_all(target).map_err(io("create directory", target))
}

fn remove_directory(app_root: &Path, target: &Path) -> Result<(), String> {
    ensure_contained(app_root, target)?;
    fs::remove_dir_all(target).map_err(io("remove directory", target))
}

fn slug(id: &str) -> String {
    id.chars()
        .map(|character| {
            if character.is_ascii_lowercase()
                || character.is_ascii_uppercase()
                || character.is_ascii_digit()
                || character == '-'
            {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn steam_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn io<'a>(action: &'a str, path: &'a Path) -> impl Fn(std::io::Error) -> String + 'a {
    move |error| format!("failed to {action} '{}': {error}", path.display())
}
