//! Installed composition resolution and safe launch preparation.
//!
//! This module verifies the complete installed Packagepack graph and resolves
//! the exact executable that the host may launch.

use super::*;
use crate::target::host_runtime_target;
use vapor_content::RuntimePresentation;

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
    #[cfg(target_os = "linux")]
    terminal_host: PathBuf,
    presentation: RuntimePresentation,
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

    /// Host presentation required by the selected Engine.
    pub fn presentation(&self) -> RuntimePresentation {
        self.presentation
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
    let presentation = engine_presentation(graph.engine.installed_root())?;
    #[cfg(target_os = "linux")]
    let terminal_host = installation
        .binaries()
        .join(format!("vapor-entrypoint{}", std::env::consts::EXE_SUFFIX));
    Ok(PackagepackLaunchPlan {
        packagepack_id: graph.root.artifact_id().to_owned(),
        packagepack_root: graph.root.installed_root().to_path_buf(),
        engine_id: graph.engine.artifact_id().to_owned(),
        engine_root: graph.engine.installed_root().to_path_buf(),
        game_id: graph.game.artifact_id().to_owned(),
        game_root: graph.game.installed_root().to_path_buf(),
        runtime_target,
        binary,
        #[cfg(target_os = "linux")]
        terminal_host,
        presentation,
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
    let mut command = match plan.presentation {
        RuntimePresentation::Window => {
            vapor_core::managed_process::background_command(&plan.binary)
        }
        RuntimePresentation::Terminal => {
            #[cfg(target_os = "linux")]
            {
                if !plan.terminal_host.is_file() {
                    return Err(format!(
                        "terminal Engine launch needs Vapor Entrypoint at {}",
                        plan.terminal_host.display()
                    ));
                }
                let mut command = Command::new(&plan.terminal_host);
                command
                    .arg("terminal")
                    .arg("--working-directory")
                    .arg(&plan.packagepack_root)
                    .arg("--")
                    .arg(&plan.binary)
                    .args(arguments);
                command
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let mut command = vapor_core::managed_process::background_command(&plan.binary);
                command.creation_flags(0x0000_0010);
                command.args(arguments);
                command
            }
            #[cfg(not(any(target_os = "linux", windows)))]
            {
                let _ = arguments;
                return Err(format!(
                    "terminal Engine launch is not supported on {}",
                    std::env::consts::OS
                ));
            }
        }
    };
    command
        .current_dir(&plan.packagepack_root)
        .env("VAPOR_LAUNCH_REQUEST", launch_request)
        .env("VAPOR_PACKAGEPACK_ID", &plan.packagepack_id)
        .env("VAPOR_PACKAGEPACK_ROOT", &plan.packagepack_root)
        .env("VAPOR_ENGINE_ID", &plan.engine_id)
        .env("VAPOR_ENGINE_ROOT", &plan.engine_root)
        .env("VAPOR_RUNTIME_TARGET", &plan.runtime_target)
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

fn engine_presentation(engine_root: &Path) -> Result<RuntimePresentation, String> {
    let manifest_path = engine_root.join(manifest::ENGINE_FILE_NAME);
    let source = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("failed to read '{}': {error}", manifest_path.display()))?;
    let parsed: LaunchManifest = toml::from_str(&source)
        .map_err(|error| format!("failed to parse '{}': {error}", manifest_path.display()))?;
    let engine = parsed.engine.ok_or_else(|| {
        format!(
            "resolved Engine manifest '{}' has no [engine] section",
            manifest_path.display()
        )
    })?;
    Ok(engine.presentation)
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
    engine: Option<LaunchContent>,
    enginepack: Option<LaunchContent>,
    gamepack: Option<LaunchContent>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
struct LaunchContent {
    #[serde(default)]
    presentation: RuntimePresentation,
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
