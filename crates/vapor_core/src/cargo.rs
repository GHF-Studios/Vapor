//! Cargo inspection and realization of Rust-backed Vapor Content.
//!
//! Vapor decides semantic Content identity and dependency structure.
//! Cargo describes and executes the physical Rust package/build graph.

use crate::{
    ContentKind, ContentVersionId, LocalCatalog, LocalContent, ManagedToolchain,
    ResolvedComposition, ToolchainError,
};
use semver::Version;
use serde::Deserialize;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

const APP_PACKAGE: &str = "vapor-generated-app";
const APP_BINARY: &str = "vapor-app";

/// Cargo-native description of the package physically associated with one
/// locally available Vapor Content artifact.
///
/// Cargo identifiers here are realization-domain information. They are not
/// Vapor semantic identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoPackageInspection {
    /// Opaque Cargo package identifier as reported by Cargo metadata.
    pub id: String,

    pub name: String,
    pub version: Version,
    pub manifest_path: PathBuf,
    pub workspace_root: PathBuf,
    pub targets: Vec<CargoTargetInspection>,
}

impl CargoPackageInspection {
    pub fn library_targets(&self) -> impl Iterator<Item = &CargoTargetInspection> {
        self.targets
            .iter()
            .filter(|target| target.is_rust_library())
    }

    pub fn has_library_target(&self) -> bool {
        self.library_targets().next().is_some()
    }
}

/// One Rust target reported by Cargo metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoTargetInspection {
    pub name: String,
    pub kind: Vec<String>,
    pub crate_types: Vec<String>,
    pub src_path: PathBuf,
}

impl CargoTargetInspection {
    /// Whether this target provides a Rust-linkable library-style interface.
    pub fn is_rust_library(&self) -> bool {
        self.kind
            .iter()
            .any(|kind| matches!(kind.as_str(), "lib" | "proc-macro"))
    }
}

/// Inspect the physical Cargo package associated with one locally discovered
/// Vapor Content project.
///
/// This intentionally asks Cargo rather than manually reconstructing Cargo's
/// workspace/package model.
///
/// Current mapping rule:
///
/// 1. Prefer the package whose manifest is exactly the Content project's
///    `Cargo.toml`.
/// 2. If the supplied Cargo manifest describes a workspace containing exactly
///    one package, use that package.
/// 3. Otherwise report ambiguity.
///
/// Explicit multi-package Vapor-to-Cargo mapping remains future pressure.
pub fn inspect_local_cargo_package(
    content: &LocalContent,
) -> Result<CargoPackageInspection, CargoInspectionError> {
    let manifest_path = content.root.join("Cargo.toml");

    if !manifest_path.is_file() {
        return Err(CargoInspectionError::MissingCargoManifest {
            identity: content.version_id(),
            path: manifest_path,
        });
    }

    let manifest_path =
        fs::canonicalize(&manifest_path).map_err(|source| CargoInspectionError::Io {
            path: manifest_path.clone(),
            source,
        })?;

    let toolchain = ManagedToolchain::discover()?;

    let output = toolchain
        .cargo_command()?
        .arg("metadata")
        .args(["--format-version", "1", "--no-deps"])
        .arg("--manifest-path")
        .arg(&manifest_path)
        .current_dir(&content.root)
        .output()
        .map_err(|source| CargoInspectionError::Io {
            path: manifest_path.clone(),
            source,
        })?;

    if !output.status.success() {
        return Err(CargoInspectionError::CargoMetadataFailed {
            manifest_path,
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    let metadata: CargoMetadataDocument =
        serde_json::from_slice(&output.stdout).map_err(|source| {
            CargoInspectionError::InvalidCargoMetadata {
                manifest_path: manifest_path.clone(),
                source,
            }
        })?;
    let workspace_root = metadata.workspace_root.clone();

    let package = select_content_package(content, &manifest_path, &metadata)?;

    Ok(CargoPackageInspection {
        id: package.id.clone(),
        name: package.name.clone(),
        version: package.version.clone(),
        manifest_path: package.manifest_path.clone(),
        workspace_root,
        targets: package
            .targets
            .iter()
            .map(|target| CargoTargetInspection {
                name: target.name.clone(),
                kind: target.kind.clone(),
                crate_types: target.crate_types.clone(),
                src_path: target.src_path.clone(),
            })
            .collect(),
    })
}

fn select_content_package<'a>(
    content: &LocalContent,
    manifest_path: &Path,
    metadata: &'a CargoMetadataDocument,
) -> Result<&'a CargoMetadataPackage, CargoInspectionError> {
    let exact: Vec<_> = metadata
        .packages
        .iter()
        .filter(|package| cargo_paths_match(&package.manifest_path, manifest_path))
        .collect();

    if let [package] = exact.as_slice() {
        return Ok(*package);
    }

    if exact.is_empty() {
        if let [package] = metadata.packages.as_slice() {
            return Ok(package);
        }
    }

    Err(CargoInspectionError::AmbiguousCargoPackage {
        identity: content.version_id(),
        manifest_path: manifest_path.to_path_buf(),
        packages: metadata
            .packages
            .iter()
            .map(|package| {
                format!(
                    "{} {} ({})",
                    package.name,
                    package.version,
                    package.manifest_path.display()
                )
            })
            .collect(),
    })
}

fn cargo_paths_match(left: &Path, right: &Path) -> bool {
    let left = fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right = fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());

    left == right
}

#[derive(Clone, Debug, Deserialize)]
struct CargoMetadataDocument {
    packages: Vec<CargoMetadataPackage>,
    workspace_root: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
struct CargoMetadataPackage {
    id: String,
    name: String,
    version: Version,
    manifest_path: PathBuf,
    targets: Vec<CargoMetadataTarget>,
}

#[derive(Clone, Debug, Deserialize)]
struct CargoMetadataTarget {
    name: String,
    kind: Vec<String>,
    crate_types: Vec<String>,
    src_path: PathBuf,
}

#[derive(Debug)]
pub enum CargoInspectionError {
    MissingCargoManifest {
        identity: ContentVersionId,
        path: PathBuf,
    },

    AmbiguousCargoPackage {
        identity: ContentVersionId,
        manifest_path: PathBuf,
        packages: Vec<String>,
    },

    InvalidCargoMetadata {
        manifest_path: PathBuf,
        source: serde_json::Error,
    },

    Toolchain(ToolchainError),

    Io {
        path: PathBuf,
        source: io::Error,
    },

    CargoMetadataFailed {
        manifest_path: PathBuf,
        status: ExitStatus,
        stderr: String,
    },
}

impl From<ToolchainError> for CargoInspectionError {
    fn from(error: ToolchainError) -> Self {
        Self::Toolchain(error)
    }
}

impl fmt::Display for CargoInspectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCargoManifest { identity, path } => {
                write!(
                    formatter,
                    "Rust-backed Vapor Content `{identity}` has no Cargo manifest at `{}`",
                    path.display()
                )
            }

            Self::AmbiguousCargoPackage {
                identity,
                manifest_path,
                packages,
            } => {
                write!(
                    formatter,
                    "cannot infer which Cargo package realizes `{identity}` from `{}`",
                    manifest_path.display()
                )?;

                if packages.is_empty() {
                    formatter.write_str(": Cargo reported no packages")
                } else {
                    formatter.write_str("; candidate packages: ")?;

                    for (index, package) in packages.iter().enumerate() {
                        if index > 0 {
                            formatter.write_str(", ")?;
                        }

                        formatter.write_str(package)?;
                    }

                    Ok(())
                }
            }

            Self::InvalidCargoMetadata {
                manifest_path,
                source,
            } => {
                write!(
                    formatter,
                    "Cargo returned invalid metadata for `{}`: {source}",
                    manifest_path.display()
                )
            }

            Self::Toolchain(error) => write!(formatter, "{error}"),

            Self::Io { path, source } => {
                write!(
                    formatter,
                    "failed to inspect `{}`: {source}",
                    path.display()
                )
            }

            Self::CargoMetadataFailed {
                manifest_path,
                status,
                stderr,
            } => {
                write!(
                    formatter,
                    "Cargo metadata failed for `{}` with {status}",
                    manifest_path.display()
                )?;

                if !stderr.is_empty() {
                    write!(formatter, ": {stderr}")?;
                }

                Ok(())
            }
        }
    }
}

impl std::error::Error for CargoInspectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidCargoMetadata { source, .. } => Some(source),
            Self::Toolchain(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoRealization {
    pub root: PathBuf,
    pub manifest_path: PathBuf,
}

pub fn generate_local_cargo_realization(
    source_root: impl AsRef<Path>,
    catalog: &LocalCatalog,
    composition: &ResolvedComposition,
) -> Result<CargoRealization, CargoRealizationError> {
    let packages = AppPackages::from_composition(catalog, composition)?;

    let root = source_root
        .as_ref()
        .join(".vapor/build")
        .join(filesystem_name(&composition.root));

    let source = root.join("src");

    fs::create_dir_all(&source).map_err(|error| io_error(&source, error))?;

    let manifest_path = root.join("Cargo.toml");

    write(&manifest_path, &packages.cargo_manifest())?;

    write(&source.join("lib.rs"), GENERATED_LIBRARY)?;

    write(&source.join("main.rs"), GENERATED_MAIN)?;

    Ok(CargoRealization {
        root,
        manifest_path,
    })
}

pub fn build_cargo_realization(
    realization: &CargoRealization,
) -> Result<(), CargoRealizationError> {
    run_cargo(realization, "build", &[])
}

pub fn run_cargo_realization(realization: &CargoRealization) -> Result<(), CargoRealizationError> {
    run_cargo(realization, "run", &["--quiet", "--bin", APP_BINARY])
}

fn run_cargo(
    realization: &CargoRealization,
    operation: &'static str,
    arguments: &[&str],
) -> Result<(), CargoRealizationError> {
    let toolchain = ManagedToolchain::discover()?;

    let status = toolchain
        .cargo_command()?
        .arg(operation)
        .args(arguments)
        .arg("--manifest-path")
        .arg("Cargo.toml")
        .current_dir(&realization.root)
        .status()
        .map_err(|error| io_error(&realization.manifest_path, error))?;

    if status.success() {
        Ok(())
    } else {
        Err(CargoRealizationError::CargoFailed { operation, status })
    }
}

struct AppPackages {
    engine: RustPackage,
    game: RustPackage,
    game_mod: RustPackage,
}

impl AppPackages {
    fn from_composition(
        catalog: &LocalCatalog,
        composition: &ResolvedComposition,
    ) -> Result<Self, CargoRealizationError> {
        let game_mods: Vec<_> = composition
            .nodes
            .values()
            .filter(|node| node.kind == ContentKind::GameMod)
            .collect();

        if game_mods.len() != 1 {
            return Err(CargoRealizationError::UnsupportedGameModCount {
                count: game_mods.len(),
            });
        }

        Ok(Self {
            engine: RustPackage::load(local(catalog, &composition.effective_engine)?)?,

            game: RustPackage::load(local(catalog, &composition.effective_game)?)?,

            game_mod: RustPackage::load(local(catalog, &game_mods[0].identity)?)?,
        })
    }

    fn cargo_manifest(&self) -> String {
        format!(
            r#"[package]
name = "{APP_PACKAGE}"
version = "0.0.0"
edition = "2024"
publish = false

[lib]
name = "vapor_app"
path = "src/lib.rs"

[[bin]]
name = "{APP_BINARY}"
path = "src/main.rs"

[dependencies]
engine = {engine}
game = {game}
game_mod = {game_mod}

[workspace]
"#,
            engine = self.engine.dependency(),
            game = self.game.dependency(),
            game_mod = self.game_mod.dependency(),
        )
    }
}

struct RustPackage {
    name: String,
    root: PathBuf,
    version: Version,
}

impl RustPackage {
    fn load(content: &LocalContent) -> Result<Self, CargoRealizationError> {
        let manifest_path = content.root.join("Cargo.toml");

        let manifest: CargoManifest = toml::from_str(&read(&manifest_path)?).map_err(|error| {
            CargoRealizationError::InvalidCargoManifest {
                path: manifest_path.clone(),
                message: error.to_string(),
            }
        })?;

        let root =
            fs::canonicalize(&content.root).map_err(|error| io_error(&content.root, error))?;

        Ok(Self {
            name: manifest.package.name,
            root,
            version: content.manifest.content.version.clone(),
        })
    }

    fn dependency(&self) -> String {
        format!(
            "{{ package = {}, path = {}, version = {} }}",
            toml_string(&self.name),
            toml_string(self.root.to_string_lossy()),
            toml_string(format!("={}", self.version)),
        )
    }
}

#[derive(Deserialize)]
struct CargoManifest {
    package: CargoPackage,
}

#[derive(Deserialize)]
struct CargoPackage {
    name: String,
}

fn local<'a>(
    catalog: &'a LocalCatalog,
    identity: &ContentVersionId,
) -> Result<&'a LocalContent, CargoRealizationError> {
    catalog
        .get(identity)
        .ok_or_else(|| CargoRealizationError::MissingLocalContent {
            identity: identity.clone(),
        })
}

fn read(path: &Path) -> Result<String, CargoRealizationError> {
    fs::read_to_string(path).map_err(|error| io_error(path, error))
}

fn write(path: &Path, contents: &str) -> Result<(), CargoRealizationError> {
    fs::write(path, contents).map_err(|error| io_error(path, error))
}

fn io_error(path: &Path, source: io::Error) -> CargoRealizationError {
    CargoRealizationError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn toml_string(value: impl AsRef<str>) -> String {
    toml::Value::String(value.as_ref().to_owned()).to_string()
}

fn filesystem_name(identity: &ContentVersionId) -> String {
    identity
        .to_string()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

const GENERATED_LIBRARY: &str = r#"//! Generated static Vapor App Composition.

pub fn run() {
    engine::run(|app| {
        game::install(app);
        game_mod::install(app);
    });
}
"#;

const GENERATED_MAIN: &str = r#"fn main() {
    vapor_app::run();
}
"#;

#[derive(Debug)]
pub enum CargoRealizationError {
    MissingLocalContent {
        identity: ContentVersionId,
    },

    UnsupportedGameModCount {
        count: usize,
    },

    InvalidCargoManifest {
        path: PathBuf,
        message: String,
    },

    Toolchain(ToolchainError),

    Io {
        path: PathBuf,
        source: io::Error,
    },

    CargoFailed {
        operation: &'static str,
        status: ExitStatus,
    },
}

impl From<ToolchainError> for CargoRealizationError {
    fn from(error: ToolchainError) -> Self {
        Self::Toolchain(error)
    }
}

impl fmt::Display for CargoRealizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingLocalContent { identity } => {
                write!(
                    formatter,
                    "resolved Vapor Content `{identity}` is not locally available"
                )
            }

            Self::UnsupportedGameModCount { count } => {
                write!(
                    formatter,
                    "Vertical Slice 0 requires exactly one Game Mod, found {count}"
                )
            }

            Self::InvalidCargoManifest { path, message } => {
                write!(
                    formatter,
                    "invalid Cargo manifest `{}`: {message}",
                    path.display()
                )
            }

            Self::Toolchain(error) => {
                write!(formatter, "{error}")
            }

            Self::Io { path, source } => {
                write!(formatter, "failed to access `{}`: {source}", path.display())
            }

            Self::CargoFailed { operation, status } => {
                write!(formatter, "Cargo `{operation}` failed with {status}")
            }
        }
    }
}

impl std::error::Error for CargoRealizationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Toolchain(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_version_becomes_filesystem_name() {
        let identity = ContentVersionId {
            id: "ghf-studios/example/packagepack".parse().unwrap(),
            version: Version::parse("1.2.3").unwrap(),
        };

        assert_eq!(
            filesystem_name(&identity),
            "ghf-studios-example-packagepack-1.2.3"
        );
    }
}
