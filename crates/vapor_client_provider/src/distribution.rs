//! Declarative assembly of the self-hosting Steam application payload.

use crate::discovery::{EnvironmentPaths, ensure_contained};
use crate::target::{host_runtime_target, validate_runtime_target};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

/// Distribution manifest filename at the app source root.
pub const FILE_NAME: &str = crate::manifest::APP_SOURCE_FILE_NAME;

/// Parsed application and payload policy.
#[derive(Debug, Clone, Deserialize)]
pub struct DistributionManifest {
    #[serde(skip)]
    _private: (),
    application: Application,
}

/// Steam application identifiers and development branch.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Application {
    app_id: u32,
    development_branch: String,
    depots: Option<SteamDepots>,
}

/// Steam depot definitions for the platform-split app payload.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SteamDepots {
    common: SteamDepot,
    linux: SteamDepot,
    windows: SteamDepot,
}

/// One logical Steam depot and its explicit source include list.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SteamDepot {
    id: u32,
    #[serde(default)]
    include: Vec<DepotInclude>,
}

/// Logical Steam depot produced by Vapor root staging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SteamDepotKind {
    /// OS-neutral shared files.
    Common,
    /// Linux runtime files and launcher.
    Linux,
    /// Windows runtime files and launcher.
    Windows,
}

/// One manifest-declared allowlisted staging input.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DepotInclude {
    root: PayloadRoot,
    from: PathBuf,
    to: PathBuf,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    exclude: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PayloadRoot {
    Installation,
    Source,
    Output,
}

/// Completed staging summary.
#[derive(Debug, Clone)]
pub struct StageReport {
    root: PathBuf,
    depots: Vec<DepotStage>,
    files: usize,
}

/// One staged depot content root.
#[derive(Debug, Clone)]
pub struct DepotStage {
    kind: SteamDepotKind,
    root: PathBuf,
}

/// Options for assembling the app/depot staging tree.
#[derive(Debug, Clone)]
pub struct StageOptions {
    runtime_targets: Vec<String>,
}

impl DistributionManifest {
    /// Load and validate the umbrella distribution manifest.
    pub fn load(paths: &EnvironmentPaths) -> Result<Self, String> {
        Self::load_optional(paths)?.ok_or_else(|| {
            format!(
                "source root '{}' does not declare [root.steam]",
                paths.source().root().display()
            )
        })
    }

    /// Load the root Steam policy when one is declared.
    ///
    /// A source root does not need to be a self-hosting Steam application,
    /// so absence is distinct from malformed distribution policy.
    pub fn load_optional(paths: &EnvironmentPaths) -> Result<Option<Self>, String> {
        let path = paths.source().root().join(FILE_NAME);
        if !path.is_file() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
        #[derive(Deserialize)]
        struct Root {
            root: Option<RootPolicy>,
        }
        #[derive(Deserialize)]
        struct RootPolicy {
            steam: Option<Application>,
        }
        let manifest = toml::from_str::<Root>(&text)
            .map_err(|error| format!("failed to parse '{}': {error}", path.display()))?;
        let Some(root) = manifest.root else {
            return Ok(None);
        };
        let Some(application) = root.steam else {
            return Ok(None);
        };
        let manifest = DistributionManifest {
            _private: (),
            application,
        };
        if manifest.application.app_id == 0 {
            return Err("Steam AppID must be non-zero".to_owned());
        }
        let Some(depots) = manifest.application.depots.as_ref() else {
            return Err(
                "root.steam.depots is required; configure common, linux, and windows depot IDs"
                    .to_owned(),
            );
        };
        depots.validate()?;
        if depots.ids().len() != 3 {
            return Err("root.steam.depots IDs must be unique".to_owned());
        }
        if manifest.application.development_branch.trim().is_empty()
            || manifest.application.development_branch == "default"
        {
            return Err("development branch must be non-empty and non-default".to_owned());
        }
        depots.validate_includes()?;
        Ok(Some(manifest))
    }

    /// Steam application policy.
    pub fn application(&self) -> &Application {
        &self.application
    }
}

impl Application {
    /// Steam AppID.
    pub fn app_id(&self) -> u32 {
        self.app_id
    }
    /// Automatically activated development beta branch.
    pub fn development_branch(&self) -> &str {
        &self.development_branch
    }
    /// Platform-split Steam depots.
    pub fn depots(&self) -> &SteamDepots {
        self.depots
            .as_ref()
            .expect("distribution manifest validation requires Steam depots")
    }
    /// Steam DepotID for one logical depot kind.
    pub fn depot_id(&self, kind: SteamDepotKind) -> u32 {
        self.depots().id(kind)
    }
    /// Steam depot definition for one logical depot kind.
    pub fn depot(&self, kind: SteamDepotKind) -> &SteamDepot {
        self.depots().depot(kind)
    }
}

impl SteamDepots {
    /// Shared content depot ID.
    pub fn common(&self) -> u32 {
        self.common.id
    }
    /// Linux runtime depot ID.
    pub fn linux(&self) -> u32 {
        self.linux.id
    }
    /// Windows runtime depot ID.
    pub fn windows(&self) -> u32 {
        self.windows.id
    }
    /// Depot ID for one logical depot kind.
    pub fn id(&self, kind: SteamDepotKind) -> u32 {
        self.depot(kind).id
    }

    /// Depot definition for one logical depot kind.
    pub fn depot(&self, kind: SteamDepotKind) -> &SteamDepot {
        match kind {
            SteamDepotKind::Common => &self.common,
            SteamDepotKind::Linux => &self.linux,
            SteamDepotKind::Windows => &self.windows,
        }
    }

    fn validate(&self) -> Result<(), String> {
        for (name, id) in [
            ("common", self.common.id),
            ("linux", self.linux.id),
            ("windows", self.windows.id),
        ] {
            if id == 0 {
                return Err(format!("root.steam.depots.{name} must be non-zero"));
            }
        }
        Ok(())
    }

    fn validate_includes(&self) -> Result<(), String> {
        for kind in [
            SteamDepotKind::Common,
            SteamDepotKind::Linux,
            SteamDepotKind::Windows,
        ] {
            let depot = self.depot(kind);
            if depot.include.is_empty() {
                return Err(format!(
                    "root.steam.depots.{}.include must not be empty",
                    kind.label()
                ));
            }
            depot.validate_includes(kind)?;
        }
        Ok(())
    }

    fn ids(&self) -> BTreeSet<u32> {
        [self.common.id, self.linux.id, self.windows.id]
            .into_iter()
            .collect()
    }
}

impl SteamDepot {
    /// Steam DepotID.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Manifest-declared include list for this depot.
    pub fn include(&self) -> &[DepotInclude] {
        &self.include
    }

    fn validate_includes(&self, kind: SteamDepotKind) -> Result<(), String> {
        for item in &self.include {
            validate_relative(&item.from)?;
            validate_relative(&item.to)?;
            for exclusion in &item.exclude {
                validate_relative(exclusion)?;
            }
            if let Some(target) = &item.target {
                validate_runtime_target(target)?;
                let target_depot = SteamDepotKind::for_runtime_target(target)?;
                if kind == SteamDepotKind::Common {
                    return Err(format!(
                        "root.steam.depots.common.include cannot be target-scoped: {target}"
                    ));
                }
                if target_depot != kind {
                    return Err(format!(
                        "root.steam.depots.{}.include target {target} belongs to the {} depot",
                        kind.label(),
                        target_depot.label()
                    ));
                }
            }
        }
        Ok(())
    }
}

impl SteamDepotKind {
    /// Stable depot label used in staged content paths.
    pub fn label(self) -> &'static str {
        match self {
            Self::Common => "common",
            Self::Linux => "linux",
            Self::Windows => "windows",
        }
    }

    /// Steamworks OS rule that should be configured for this depot.
    pub fn steam_os_rule(self) -> &'static str {
        match self {
            Self::Common => "All OS",
            Self::Linux => "Linux",
            Self::Windows => "Windows",
        }
    }

    /// Depot kind for a supported Rust runtime target triple.
    pub fn for_runtime_target(target: &str) -> Result<Self, String> {
        if target.contains("linux") {
            Ok(Self::Linux)
        } else if target.contains("windows") {
            Ok(Self::Windows)
        } else {
            Err(format!(
                "runtime target has no configured Steam depot: {target}\nhelp: add a target-to-depot rule before staging this platform"
            ))
        }
    }
}

impl StageReport {
    /// Staged app content root containing one directory per depot.
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Staged depot content roots.
    pub fn depots(&self) -> &[DepotStage] {
        &self.depots
    }
    /// Find a staged depot by kind.
    pub fn depot(&self, kind: SteamDepotKind) -> Option<&DepotStage> {
        self.depots.iter().find(|depot| depot.kind == kind)
    }
    /// Number of copied files.
    pub fn files(&self) -> usize {
        self.files
    }
}

impl DepotStage {
    /// Logical depot kind.
    pub fn kind(&self) -> SteamDepotKind {
        self.kind
    }
    /// Staged content root for this depot.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl StageOptions {
    /// Stage only the runtime application payload.
    pub fn runtime() -> Self {
        Self {
            runtime_targets: vec![host_runtime_target()],
        }
    }

    /// Use an explicit runtime target set for target-scoped launch payloads.
    pub fn with_runtime_targets(mut self, targets: Vec<String>) -> Self {
        self.runtime_targets = if targets.is_empty() {
            vec![host_runtime_target()]
        } else {
            targets
        };
        self
    }

    /// Runtime target triples represented by this staged payload.
    pub fn runtime_targets(&self) -> &[String] {
        &self.runtime_targets
    }
}

/// Rebuild the clean, allowlisted Steam depot staging tree.
pub fn stage(
    paths: &EnvironmentPaths,
    manifest: &DistributionManifest,
) -> Result<StageReport, String> {
    stage_with_options(paths, manifest, StageOptions::runtime())
}

/// Rebuild the clean, allowlisted Steam depot staging tree with explicit options.
pub fn stage_with_options(
    paths: &EnvironmentPaths,
    manifest: &DistributionManifest,
    options: StageOptions,
) -> Result<StageReport, String> {
    let output = paths.installation().output_root();
    let root = output.join("root/content");
    ensure_contained(&output, &root)?;
    if root.exists() {
        fs::remove_dir_all(&root).map_err(io("reset staging", &root))?;
    }
    fs::create_dir_all(&root).map_err(io("create staging", &root))?;

    let mut files = 0;
    let mut depots = vec![DepotStage {
        kind: SteamDepotKind::Common,
        root: depot_root(&root, SteamDepotKind::Common),
    }];
    for kind in depot_kinds_for_targets(options.runtime_targets())? {
        depots.push(DepotStage {
            kind,
            root: depot_root(&root, kind),
        });
    }

    for depot in &depots {
        for item in manifest.application().depot(depot.kind).include() {
            files += copy_include(paths, depot.root(), item, options.runtime_targets())?;
        }
    }

    project_default_app_manifests(&depots, options.runtime_targets())?;
    validate_default_app_payload(&depots, options.runtime_targets())?;

    Ok(StageReport {
        root,
        depots,
        files,
    })
}

/// Workshop packages may include static Rust archives for creators. The
/// depot's linked default Player only declares files actually shipped there.
fn project_default_app_manifests(depots: &[DepotStage], targets: &[String]) -> Result<(), String> {
    let common = depots
        .iter()
        .find(|depot| depot.kind == SteamDepotKind::Common)
        .ok_or("staged depots have no common payload")?;
    let manifest = common.root.join(crate::manifest::APP_FILE_NAME);
    let source =
        fs::read_to_string(&manifest).map_err(io("read staged app manifest", &manifest))?;
    let app: toml::Value = toml::from_str(&source).map_err(|error| {
        format!(
            "failed to parse staged app manifest '{}': {error}",
            manifest.display()
        )
    })?;
    let Some(seeds) = app
        .get("root")
        .and_then(|root| root.get("content"))
        .and_then(toml::Value::as_array)
    else {
        return Ok(());
    };
    for seed in seeds {
        let id = seed
            .get("id")
            .and_then(toml::Value::as_str)
            .ok_or("default App seed has no content ID")?;
        let kind = seed
            .get("kind")
            .and_then(toml::Value::as_str)
            .ok_or("default App seed has no content kind")?;
        let kind = match kind {
            "engine" => crate::manifest::ContentKind::Engine,
            "game" => crate::manifest::ContentKind::Game,
            "packagepack" => crate::manifest::ContentKind::Packagepack,
            other => return Err(format!("unsupported default App content kind '{other}'")),
        };
        let relative = checked_content_id_path(id)?;
        let root = common.root.join("content/default").join(relative);
        crate::content::project_depot_runtime_manifest(&root, kind, targets)?;
    }
    Ok(())
}

/// The default launch must be complete in the staged depots themselves. In
/// particular, an app-root Workshop cache cannot satisfy this release gate.
fn validate_default_app_payload(depots: &[DepotStage], targets: &[String]) -> Result<(), String> {
    #[derive(Deserialize)]
    struct AppManifest {
        root: AppRoot,
    }
    #[derive(Deserialize)]
    struct AppRoot {
        #[serde(default)]
        content: Vec<Seed>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "kebab-case")]
    struct Seed {
        id: String,
        kind: String,
        default_launch: Option<String>,
    }
    #[derive(Deserialize)]
    struct PackagepackManifest {
        packagepack: Packagepack,
    }
    #[derive(Deserialize)]
    struct Packagepack {
        id: String,
        #[serde(default)]
        runtime: Vec<Runtime>,
    }
    #[derive(Deserialize)]
    struct Runtime {
        target: String,
        #[serde(default)]
        binaries: Vec<String>,
    }

    let common = depots
        .iter()
        .find(|depot| depot.kind == SteamDepotKind::Common)
        .ok_or("staged depots have no common payload")?;
    let app_manifest = common.root.join(crate::manifest::APP_FILE_NAME);
    let source = fs::read_to_string(&app_manifest).map_err(|error| {
        format!(
            "failed to read staged app manifest '{}': {error}",
            app_manifest.display()
        )
    })?;
    let app: AppManifest = toml::from_str(&source).map_err(|error| {
        format!(
            "failed to parse staged app manifest '{}': {error}",
            app_manifest.display()
        )
    })?;
    if app.root.content.is_empty() {
        return Ok(());
    }
    let default = app
        .root
        .content
        .iter()
        .find(|seed| seed.default_launch.as_deref() == Some("default"))
        .ok_or("staged app manifest has no default launch Packagepack")?;
    if default.kind != "packagepack" {
        return Err(format!(
            "default launch '{}' is not a Packagepack",
            default.id
        ));
    }
    let mut staged_artifacts = std::collections::BTreeMap::new();
    for seed in &app.root.content {
        let relative = checked_content_id_path(&seed.id)?;
        let manifest_name = match seed.kind.as_str() {
            "engine" => "Engine.vapor.toml",
            "game" => "Game.vapor.toml",
            "packagepack" => "Packagepack.vapor.toml",
            kind => {
                return Err(format!(
                    "unsupported default App content kind '{kind}' for {}",
                    seed.id
                ));
            }
        };
        let staged_root = common.root.join("content/default").join(relative);
        let staged = staged_root.join(manifest_name);
        if !staged.is_file() {
            return Err(format!(
                "default App release is missing staged content manifest: {}",
                staged.display()
            ));
        }
        let artifact = crate::content::read_depot_artifact(&staged_root)?;
        if artifact.id() != seed.id || artifact.kind().to_string() != seed.kind {
            return Err(format!(
                "staged default App artifact '{}' does not match its app seed",
                staged.display()
            ));
        }
        if artifact.version().is_none() {
            return Err(format!(
                "staged default App artifact '{}' has no exact version",
                staged.display()
            ));
        }
        staged_artifacts.insert(seed.id.clone(), artifact);
    }
    for artifact in staged_artifacts.values() {
        for dependency in artifact
            .dependencies()
            .iter()
            .filter(|dependency| !dependency.optional())
        {
            if !staged_artifacts.contains_key(dependency.id()) {
                return Err(format!(
                    "default App artifact '{}' requires '{}' outside the depot runtime closure",
                    artifact.id(),
                    dependency.id()
                ));
            }
        }
        let relative = checked_content_id_path(artifact.id())?;
        let artifact_root = common.root.join("content/default").join(relative);
        for target in targets {
            let kind = SteamDepotKind::for_runtime_target(target)?;
            let depot = depots
                .iter()
                .find(|depot| depot.kind == kind)
                .ok_or_else(|| format!("staged depots have no {} payload", kind.label()))?;
            for file in crate::content::depot_runtime_files(&artifact_root, target)? {
                let staged = depot.root.join("content/default").join(relative).join(file);
                if !staged.is_file() {
                    return Err(format!(
                        "default App release is missing declared runtime output: {}",
                        staged.display()
                    ));
                }
            }
        }
    }
    let default_relative = checked_content_id_path(&default.id)?;
    let packagepack_manifest = common
        .root
        .join("content/default")
        .join(default_relative)
        .join("Packagepack.vapor.toml");
    let packagepack_source = fs::read_to_string(&packagepack_manifest).map_err(|error| {
        format!(
            "failed to read staged Packagepack '{}': {error}",
            packagepack_manifest.display()
        )
    })?;
    let packagepack: PackagepackManifest =
        toml::from_str(&packagepack_source).map_err(|error| {
            format!(
                "failed to parse staged Packagepack '{}': {error}",
                packagepack_manifest.display()
            )
        })?;
    if packagepack.packagepack.id != default.id {
        return Err(format!(
            "staged default Packagepack declares '{}' instead of '{}'",
            packagepack.packagepack.id, default.id
        ));
    }
    for target in targets {
        let kind = SteamDepotKind::for_runtime_target(target)?;
        let depot = depots
            .iter()
            .find(|depot| depot.kind == kind)
            .ok_or_else(|| format!("staged depots have no {} payload", kind.label()))?;
        let binary_dir = depot
            .root
            .join("content/default")
            .join(default_relative)
            .join("bin")
            .join(target);
        let runtime = packagepack
            .packagepack
            .runtime
            .iter()
            .find(|runtime| runtime.target == *target)
            .ok_or_else(|| {
                format!("default App Packagepack has no runtime declaration for {target}")
            })?;
        if runtime.binaries.is_empty() {
            return Err(format!(
                "default App Packagepack declares no executable for {target}"
            ));
        }
        for name in &runtime.binaries {
            let path = Path::new(name);
            if path.components().count() != 1 {
                return Err(format!(
                    "default App runtime binary name is invalid: {name}"
                ));
            }
            let binary = binary_dir.join(path);
            if !binary.is_file() {
                return Err(format!(
                    "default App release is missing executable for {target}: {}",
                    binary.display()
                ));
            }
        }
    }
    Ok(())
}

fn checked_content_id_path(id: &str) -> Result<&Path, String> {
    id.parse::<vapor_content::VaporId>()
        .map_err(|error| format!("invalid default App content ID '{id}': {error}"))?;
    let path = Path::new(id);
    validate_relative(path)?;
    if path.components().count() < 2 {
        return Err(format!("content ID is not namespaced: {id}"));
    }
    Ok(path)
}

fn copy_include(
    paths: &EnvironmentPaths,
    depot_root: &Path,
    item: &DepotInclude,
    selected_targets: &[String],
) -> Result<usize, String> {
    if let Some(target) = &item.target {
        validate_runtime_target(target)?;
        if !selected_targets.iter().any(|selected| selected == target) {
            return Ok(0);
        }
    }

    let output_root = paths.installation().output_root();
    let base = match item.root {
        PayloadRoot::Installation => paths.installation().root(),
        PayloadRoot::Source => paths.source().root(),
        PayloadRoot::Output => output_root.as_path(),
    };
    let source = base.join(&item.from);
    if !source.exists() {
        if item.required {
            return Err(format!(
                "required depot include is missing: {}",
                source.display()
            ));
        }
        return Ok(0);
    }
    let canonical = fs::canonicalize(&source).map_err(io("resolve depot include", &source))?;
    ensure_contained(base, &canonical)?;
    copy_tree(
        &canonical,
        &depot_root.join(&item.to),
        &canonical,
        &item.exclude,
    )
}

fn depot_root(stage_root: &Path, kind: SteamDepotKind) -> PathBuf {
    stage_root.join(kind.label())
}

fn depot_kinds_for_targets(targets: &[String]) -> Result<BTreeSet<SteamDepotKind>, String> {
    let mut depots = BTreeSet::new();
    for target in targets {
        depots.insert(target_depot_kind(target)?);
    }
    Ok(depots)
}

fn target_depot_kind(target: &str) -> Result<SteamDepotKind, String> {
    SteamDepotKind::for_runtime_target(target)
}

fn copy_tree(
    source: &Path,
    target: &Path,
    item_root: &Path,
    exclusions: &[PathBuf],
) -> Result<usize, String> {
    let relative = source.strip_prefix(item_root).unwrap_or(Path::new(""));
    if exclusions
        .iter()
        .any(|excluded| relative.starts_with(excluded))
    {
        return Ok(0);
    }
    let canonical = fs::canonicalize(source).map_err(io("resolve payload entry", source))?;
    ensure_contained(item_root, &canonical)?;
    let metadata = fs::metadata(&canonical).map_err(io("inspect payload", &canonical))?;
    if metadata.is_dir() {
        fs::create_dir_all(target).map_err(io("create payload directory", target))?;
        let mut files = 0;
        for entry in fs::read_dir(&canonical).map_err(io("read payload directory", &canonical))? {
            let entry = entry.map_err(|error| format!("failed to read payload entry: {error}"))?;
            files += copy_tree(
                &entry.path(),
                &target.join(entry.file_name()),
                item_root,
                exclusions,
            )?;
        }
        Ok(files)
    } else if metadata.is_file() {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io("create payload parent", parent))?;
        }
        fs::copy(&canonical, target).map_err(io("copy payload", &canonical))?;
        Ok(1)
    } else {
        Ok(0)
    }
}

fn validate_relative(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        Err(format!(
            "distribution path must be safe and relative: {}",
            path.display()
        ))
    } else {
        Ok(())
    }
}

fn io<'a>(action: &'a str, path: &'a Path) -> impl Fn(std::io::Error) -> String + 'a {
    move |error| format!("failed to {action} '{}': {error}", path.display())
}

#[cfg(test)]
mod tests {
    use super::{DepotStage, SteamDepotKind, validate_default_app_payload};
    use std::fs;

    #[test]
    fn default_app_release_requires_its_declared_runtime_binary() {
        let root = std::env::temp_dir().join(format!(
            "vapor-default-depot-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let common = root.join("common");
        let linux = root.join("linux");
        fs::create_dir_all(&common).unwrap();
        fs::write(
            common.join("App.vapor.toml"),
            "[root]\n[[root.content]]\nid = \"ghf-studios/loo-cast/default\"\nkind = \"packagepack\"\ndefault-launch = \"default\"\n",
        ).unwrap();
        let packagepack = common.join("content/default/ghf-studios/loo-cast/default");
        fs::create_dir_all(&packagepack).unwrap();
        fs::write(
            packagepack.join("Packagepack.vapor.toml"),
            "schema = 1\n[packagepack]\nid = \"ghf-studios/loo-cast/default\"\nname = \"default\"\nversion = \"1.0.0\"\n[[packagepack.runtime]]\ntarget = \"x86_64-unknown-linux-gnu\"\nbinaries = [\"game\"]\n",
        ).unwrap();
        let depots = [
            DepotStage {
                kind: SteamDepotKind::Common,
                root: common,
            },
            DepotStage {
                kind: SteamDepotKind::Linux,
                root: linux.clone(),
            },
        ];
        let targets = ["x86_64-unknown-linux-gnu".to_owned()];
        assert!(validate_default_app_payload(&depots, &targets).is_err());
        let binary = linux
            .join("content/default/ghf-studios/loo-cast/default/bin/x86_64-unknown-linux-gnu/game");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, "runtime").unwrap();
        assert!(validate_default_app_payload(&depots, &targets).is_ok());
        fs::remove_dir_all(root).unwrap();
    }
}
