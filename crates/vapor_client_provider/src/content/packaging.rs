//! Convert authored content projects into app-owned installable packages.
//!
//! This boundary owns runtime target selection, compiled executable/library
//! staging, and the deployed content manifest format. Install and launch paths
//! consume its deployed artifact reader rather than interpreting source manifests
//! independently.

use super::{
    CONTENT_STATE_SCHEMA, ContentArtifact, ContentKind, ContentLayout, ContentReference,
    PackageReport, RuntimePayload, WorkshopPolicy, copy_tree, default_exclusions, discover,
    fingerprint_tree, io, reset_directory, slug, validate, write_receipt,
};
use crate::target::{host_runtime_target, validate_runtime_target};
use crate::{cross_toolchain, discovery::EnvironmentPaths, manifest};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use vapor_content::RuntimePresentation;
use vapor_content::authored_manifest::{AuthoredContentManifest, AuthoredVersion};

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

pub(super) fn package_with_runtime_targets(
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
        let temporary_root = TemporaryDirectory::new(
            layout.output_temporary(),
            "vapor-content-package",
            artifact.id(),
        )?;
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

pub(super) fn read_authored_content(
    path: &Path,
    kind: ContentKind,
) -> Result<AuthoredContent, String> {
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
        presentation: authored.presentation(),
        workshop: authored.workshop.clone(),
        dependencies: authored
            .dependency_edges(kind)
            .iter()
            .map(|edge| ContentReference::from_authored(&edge.reference, &edge.relationship))
            .collect(),
        conflicts: authored
            .conflicts
            .iter()
            .map(|reference| ContentReference::from_authored(reference, &reference.relationship))
            .collect(),
    };
    Ok(content)
}

#[derive(Debug)]
pub(super) struct AuthoredContent {
    pub(super) version: AuthoredVersion,
    pub(super) binaries: Vec<String>,
    pub(super) libraries: Vec<String>,
    pub(super) presentation: RuntimePresentation,
    pub(super) workshop: WorkshopPolicy,
    pub(super) dependencies: Vec<ContentReference>,
    pub(super) conflicts: Vec<ContentReference>,
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
    #[serde(default, skip_serializing_if = "RuntimePresentation::is_window")]
    presentation: RuntimePresentation,
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
            presentation: artifact.presentation(),
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

pub(super) fn stage_deployed_artifact(
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
pub(super) struct RuntimeTarget {
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
                    explicit: target != host_runtime_target(),
                })
            }
            None => Ok(Self::host()),
        }
    }

    pub(super) fn many(target_triples: &[String]) -> Result<Vec<Self>, String> {
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

fn binary_suffix(target: &str) -> &'static str {
    if target.contains("windows") {
        ".exe"
    } else {
        ""
    }
}

fn library_file_names(stem: &str, target: &str) -> Vec<String> {
    if target.contains("windows") {
        let mut names = vec![format!("{stem}.dll"), format!("{stem}.lib")];
        if target.contains("windows-gnu") {
            names.extend([
                format!("lib{stem}.dll.a"),
                format!("lib{stem}.a"),
                format!("lib{stem}.rlib"),
            ]);
        }
        names
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

pub(super) fn validate_runtime_output_names(label: &str, names: &[String]) -> Result<(), String> {
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

pub(super) fn read_deployed_manifest(package_root: &Path) -> Result<ContentArtifact, String> {
    let path = manifest::content_marker_at(package_root).ok_or_else(|| {
        format!(
            "deployed content root '{}' has no role-specific content manifest",
            package_root.display()
        )
    })?;
    let kind = ContentKind::ALL
        .into_iter()
        .find(|kind| {
            path.file_name().and_then(|name| name.to_str()) == Some(kind.manifest_file_name())
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
    let authored = manifest.content(kind).ok_or_else(|| {
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
        presentation: authored.presentation(),
        dependencies,
        conflicts,
        workshop: authored.workshop.clone(),
    })
}

pub(super) struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    pub(super) fn new(base: &Path, prefix: &str, artifact_id: &str) -> Result<Self, String> {
        fs::create_dir_all(base).map_err(io("create temporary staging root", base))?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
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

    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod target_tests {
    use super::{RuntimeTarget, TemporaryDirectory, library_file_names};
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn explicit_host_target_uses_default_build_output_layout() {
        let host = crate::host_runtime_target();
        let target = RuntimeTarget::many(std::slice::from_ref(&host))
            .expect("valid host target")
            .remove(0);
        assert!(!target.explicit);
        assert_eq!(target.triple(), host);
    }

    #[test]
    fn windows_gnu_crates_resolve_static_archives_and_import_libraries() {
        assert_eq!(
            library_file_names("wheel_core", "x86_64-pc-windows-gnullvm"),
            [
                "wheel_core.dll",
                "wheel_core.lib",
                "libwheel_core.dll.a",
                "libwheel_core.a",
                "libwheel_core.rlib",
            ]
        );
        assert_eq!(
            library_file_names("wheel_core", "x86_64-pc-windows-msvc"),
            ["wheel_core.dll", "wheel_core.lib"]
        );
    }

    #[test]
    fn temporary_package_staging_stays_under_the_selected_output_root() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_nanos();
        let output_root = std::env::temp_dir().join(format!(
            "vapor-provider-output-test-{}-{stamp}",
            std::process::id()
        ));
        let staging = TemporaryDirectory::new(&output_root, "package-review", "org.game")
            .expect("temporary package directory is created");
        let staging_path = staging.path().to_path_buf();

        assert!(staging_path.starts_with(&output_root));
        assert!(staging_path.is_dir());

        drop(staging);

        assert!(!staging_path.exists());
        fs::remove_dir(output_root).expect("empty output root is removed");
    }
}
