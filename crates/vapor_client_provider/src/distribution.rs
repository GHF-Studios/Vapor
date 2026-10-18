//! Declarative assembly of the self-hosting Steam application payload.

use crate::{
    discovery::{EnvironmentPaths, ensure_contained},
};
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
    let root = paths.installation().root().join("output/root/content");
    ensure_contained(paths.installation().root(), &root)?;
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

    Ok(StageReport {
        root,
        depots,
        files,
    })
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

    let base = match item.root {
        PayloadRoot::Installation => paths.installation().root(),
        PayloadRoot::Source => paths.source().root(),
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

fn host_runtime_target() -> String {
    let arch = std::env::consts::ARCH;
    match (arch, std::env::consts::OS, std::env::consts::FAMILY) {
        ("x86_64", "linux", _) => "x86_64-unknown-linux-gnu".to_owned(),
        ("aarch64", "linux", _) => "aarch64-unknown-linux-gnu".to_owned(),
        ("x86_64", "windows", _) => "x86_64-pc-windows-gnullvm".to_owned(),
        ("aarch64", "windows", _) => "aarch64-pc-windows-msvc".to_owned(),
        ("x86_64", "macos", _) => "x86_64-apple-darwin".to_owned(),
        ("aarch64", "macos", _) => "aarch64-apple-darwin".to_owned(),
        _ => format!("{arch}-{}-{}", std::env::consts::OS, std::env::consts::FAMILY),
    }
}
