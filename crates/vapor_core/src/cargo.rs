//! Cargo realization of an exact resolved Vapor App Composition.
//!
//! Vapor decides what is built. Cargo performs the Rust build.

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
    engine::run(game::run, game_mod::run);
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
