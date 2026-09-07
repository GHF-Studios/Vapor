use crate::installation::VaporInstallation;
use crate::registry::{RegisteredRepository, RegistryClient, RegistryError};
use crate::role::git_available;
use crate::source::{SourceError, open_source};
use crate::steam::{INSTALLED_ECOSYSTEM_METADATA_FILE_NAME, ROOT_DISTRIBUTION_MANIFEST_FILE_NAME};
use crate::superworkspace::{SuperworkspaceError, VaporSuperworkspace};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus};

#[derive(Debug, Clone)]
pub struct EcosystemBootstrap {
    pub ecosystem_id: String,
    pub registry_endpoint: String,
}

#[derive(Debug, Clone)]
pub struct EcosystemAcquisitionReport {
    pub ecosystem_id: String,
    pub registry_endpoint: String,
    pub superworkspace_root: PathBuf,
    pub repositories: Vec<AcquiredRepository>,
}

#[derive(Debug, Clone)]
pub struct AcquiredRepository {
    pub id: String,
    pub root: PathBuf,
}

#[derive(Debug, Deserialize)]
struct BootstrapDocument {
    ecosystem: Option<BootstrapEcosystem>,
    registry: Option<BootstrapRegistry>,
}

#[derive(Debug, Deserialize)]
struct BootstrapEcosystem {
    id: String,
}

#[derive(Debug, Deserialize)]
struct BootstrapRegistry {
    endpoint: String,
}

pub fn install_ecosystem_bootstrap(
    installation: &VaporInstallation,
    source_root: &Path,
) -> Result<PathBuf, EcosystemError> {
    let source = find_source_bootstrap(source_root)?;

    // Validate before installing it.
    load_bootstrap_file(&source)?.ok_or_else(|| EcosystemError::IncompleteBootstrap {
        path: source.clone(),
    })?;

    let metadata_root = installation.root.join("metadata");

    fs::create_dir_all(&metadata_root).map_err(|source| EcosystemError::Io {
        path: metadata_root.clone(),
        source,
    })?;

    let destination = metadata_root.join(INSTALLED_ECOSYSTEM_METADATA_FILE_NAME);

    fs::copy(&source, &destination).map_err(|source| EcosystemError::Io {
        path: destination.clone(),
        source,
    })?;

    Ok(destination)
}

pub fn acquire_ecosystem(
    installation: &VaporInstallation,
    destination: &Path,
) -> Result<EcosystemAcquisitionReport, EcosystemError> {
    if !git_available() {
        return Err(EcosystemError::GitUnavailable);
    }

    let bootstrap = load_bootstrap(installation)?;

    let (namespace, name) = split_ecosystem_id(&bootstrap.ecosystem_id)?;

    let registry = RegistryClient::new(&bootstrap.registry_endpoint)?;

    let ecosystem = registry.ecosystem(namespace, name)?;

    if ecosystem.repositories.is_empty() {
        return Err(EcosystemError::NoRepositories {
            ecosystem: ecosystem.id,
        });
    }

    let destination = prepare_destination(installation, destination)?;

    let mut checkout_paths = BTreeSet::new();
    let mut acquired = Vec::new();

    for repository in &ecosystem.repositories {
        validate_repository(repository)?;

        let checkout_path = checkout_path(repository)?;

        if !checkout_paths.insert(checkout_path.clone()) {
            return Err(EcosystemError::DuplicateCheckoutPath(checkout_path));
        }

        let target = destination.join(checkout_path);

        if target.exists() {
            return Err(EcosystemError::CheckoutAlreadyExists {
                repository: repository.id.clone(),
                path: target,
            });
        }

        clone_repository(repository, &target)?;
        initialize_submodules(repository, &target)?;

        acquired.push(AcquiredRepository {
            id: repository.id.clone(),
            root: target,
        });
    }

    let superworkspace =
        VaporSuperworkspace::discover_from(&destination).map_err(EcosystemError::Superworkspace)?;

    let discovered =
        fs::canonicalize(&superworkspace.root).unwrap_or_else(|_| superworkspace.root.clone());

    if discovered != destination {
        return Err(EcosystemError::UnexpectedSuperworkspace {
            expected: destination,
            discovered,
        });
    }

    open_source(installation, &superworkspace.root).map_err(EcosystemError::Source)?;

    Ok(EcosystemAcquisitionReport {
        ecosystem_id: ecosystem.id,
        registry_endpoint: registry.endpoint().to_owned(),
        superworkspace_root: superworkspace.root,
        repositories: acquired,
    })
}

fn load_bootstrap(installation: &VaporInstallation) -> Result<EcosystemBootstrap, EcosystemError> {
    let installed = installation
        .root
        .join("metadata")
        .join(INSTALLED_ECOSYSTEM_METADATA_FILE_NAME);

    if let Some(bootstrap) = load_bootstrap_file(&installed)? {
        return Ok(bootstrap);
    }

    let current = env::current_dir().map_err(EcosystemError::CurrentDirectory)?;

    if let Ok(source) = find_source_bootstrap(&current)
        && let Some(bootstrap) = load_bootstrap_file(&source)?
    {
        return Ok(bootstrap);
    }

    Err(EcosystemError::BootstrapUnavailable { installed })
}

fn load_bootstrap_file(path: &Path) -> Result<Option<EcosystemBootstrap>, EcosystemError> {
    if !path.is_file() {
        return Ok(None);
    }

    let source = fs::read_to_string(path).map_err(|source| EcosystemError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    let document: BootstrapDocument =
        toml::from_str(&source).map_err(|error| EcosystemError::InvalidBootstrap {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;

    let (Some(ecosystem), Some(registry)) = (document.ecosystem, document.registry) else {
        return Ok(None);
    };

    split_ecosystem_id(&ecosystem.id)?;

    if registry.endpoint.trim().is_empty() {
        return Err(EcosystemError::InvalidBootstrap {
            path: path.to_path_buf(),
            message: "Registry endpoint is empty".to_owned(),
        });
    }

    Ok(Some(EcosystemBootstrap {
        ecosystem_id: ecosystem.id,
        registry_endpoint: registry.endpoint,
    }))
}

fn find_source_bootstrap(start: &Path) -> Result<PathBuf, EcosystemError> {
    let start = fs::canonicalize(start).map_err(|source| EcosystemError::Io {
        path: start.to_path_buf(),
        source,
    })?;

    for root in start.ancestors() {
        let candidate = root.join(ROOT_DISTRIBUTION_MANIFEST_FILE_NAME);

        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(EcosystemError::SourceBootstrapNotFound { start })
}

fn split_ecosystem_id(id: &str) -> Result<(&str, &str), EcosystemError> {
    let Some((namespace, name)) = id.split_once('/') else {
        return Err(EcosystemError::InvalidEcosystemId(id.to_owned()));
    };

    if namespace.is_empty() || name.is_empty() || name.contains('/') {
        return Err(EcosystemError::InvalidEcosystemId(id.to_owned()));
    }

    Ok((namespace, name))
}

fn prepare_destination(
    installation: &VaporInstallation,
    destination: &Path,
) -> Result<PathBuf, EcosystemError> {
    if destination.exists() && !destination.is_dir() {
        return Err(EcosystemError::DestinationNotDirectory(
            destination.to_path_buf(),
        ));
    }

    fs::create_dir_all(destination).map_err(|source| EcosystemError::Io {
        path: destination.to_path_buf(),
        source,
    })?;

    let destination = fs::canonicalize(destination).map_err(|source| EcosystemError::Io {
        path: destination.to_path_buf(),
        source,
    })?;

    let installation =
        fs::canonicalize(&installation.root).unwrap_or_else(|_| installation.root.clone());

    if destination.starts_with(&installation) || installation.starts_with(&destination) {
        return Err(EcosystemError::OverlapsInstallation {
            destination,
            installation,
        });
    }

    Ok(destination)
}

fn validate_repository(repository: &RegisteredRepository) -> Result<(), EcosystemError> {
    if repository.provider != "github" {
        return Err(EcosystemError::UnsupportedProvider {
            repository: repository.id.clone(),
            provider: repository.provider.clone(),
        });
    }

    if repository.kind != "container-repo" {
        return Err(EcosystemError::UnsupportedRepositoryKind {
            repository: repository.id.clone(),
            kind: repository.kind.clone(),
        });
    }

    Ok(())
}

fn checkout_path(repository: &RegisteredRepository) -> Result<PathBuf, EcosystemError> {
    let path = Path::new(&repository.checkout_path);
    let mut components = path.components();

    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(path.to_path_buf()),

        _ => Err(EcosystemError::UnsafeCheckoutPath {
            repository: repository.id.clone(),
            path: repository.checkout_path.clone(),
        }),
    }
}

fn clone_repository(
    repository: &RegisteredRepository,
    target: &Path,
) -> Result<(), EcosystemError> {
    let status = Command::new("git")
        .arg("clone")
        .arg("--")
        .arg(&repository.clone_url)
        .arg(target)
        .status()
        .map_err(|source| EcosystemError::GitStart {
            repository: repository.id.clone(),
            operation: "clone",
            source,
        })?;

    ensure_git_success(repository, "clone", status)
}

fn initialize_submodules(
    repository: &RegisteredRepository,
    target: &Path,
) -> Result<(), EcosystemError> {
    let status = Command::new("git")
        .arg("-C")
        .arg(target)
        .args(["submodule", "update", "--init"])
        .status()
        .map_err(|source| EcosystemError::GitStart {
            repository: repository.id.clone(),
            operation: "submodule initialization",
            source,
        })?;

    ensure_git_success(repository, "submodule initialization", status)
}

fn ensure_git_success(
    repository: &RegisteredRepository,
    operation: &'static str,
    status: ExitStatus,
) -> Result<(), EcosystemError> {
    if status.success() {
        Ok(())
    } else {
        Err(EcosystemError::GitFailed {
            repository: repository.id.clone(),
            operation,
            status,
        })
    }
}

#[derive(Debug)]
pub enum EcosystemError {
    Registry(RegistryError),

    CurrentDirectory(io::Error),

    GitUnavailable,

    BootstrapUnavailable {
        installed: PathBuf,
    },

    SourceBootstrapNotFound {
        start: PathBuf,
    },

    IncompleteBootstrap {
        path: PathBuf,
    },

    InvalidBootstrap {
        path: PathBuf,
        message: String,
    },

    InvalidEcosystemId(String),

    NoRepositories {
        ecosystem: String,
    },

    DestinationNotDirectory(PathBuf),

    CheckoutAlreadyExists {
        repository: String,
        path: PathBuf,
    },

    OverlapsInstallation {
        destination: PathBuf,
        installation: PathBuf,
    },

    UnsupportedProvider {
        repository: String,
        provider: String,
    },

    UnsupportedRepositoryKind {
        repository: String,
        kind: String,
    },

    UnsafeCheckoutPath {
        repository: String,
        path: String,
    },

    DuplicateCheckoutPath(PathBuf),

    GitStart {
        repository: String,
        operation: &'static str,
        source: io::Error,
    },

    GitFailed {
        repository: String,
        operation: &'static str,
        status: ExitStatus,
    },

    Superworkspace(SuperworkspaceError),

    UnexpectedSuperworkspace {
        expected: PathBuf,
        discovered: PathBuf,
    },

    Source(SourceError),

    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl From<RegistryError> for EcosystemError {
    fn from(error: RegistryError) -> Self {
        Self::Registry(error)
    }
}

impl fmt::Display for EcosystemError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(error) => error.fmt(formatter),

            Self::CurrentDirectory(error) => {
                write!(formatter, "failed to determine current directory: {error}")
            }

            Self::GitUnavailable => {
                formatter.write_str("Git is required to acquire Vapor ecosystem source")
            }

            Self::BootstrapUnavailable { installed } => {
                write!(
                    formatter,
                    "Vapor ecosystem bootstrap metadata is unavailable at `{}`",
                    installed.display()
                )
            }

            Self::SourceBootstrapNotFound { start } => {
                write!(
                    formatter,
                    "could not find `{ROOT_DISTRIBUTION_MANIFEST_FILE_NAME}` from `{}`",
                    start.display()
                )
            }

            Self::IncompleteBootstrap { path } => {
                write!(
                    formatter,
                    "ecosystem bootstrap `{}` does not declare ecosystem and Registry identity",
                    path.display()
                )
            }

            Self::InvalidBootstrap { path, message } => {
                write!(
                    formatter,
                    "invalid ecosystem bootstrap `{}`: {message}",
                    path.display()
                )
            }

            Self::InvalidEcosystemId(id) => {
                write!(
                    formatter,
                    "invalid ecosystem identity `{id}`; expected `namespace/name`"
                )
            }

            Self::NoRepositories { ecosystem } => {
                write!(
                    formatter,
                    "Registry declares no top-level repositories for `{ecosystem}`"
                )
            }

            Self::DestinationNotDirectory(path) => {
                write!(
                    formatter,
                    "acquisition destination is not a directory: `{}`",
                    path.display()
                )
            }

            Self::CheckoutAlreadyExists { repository, path } => {
                write!(
                    formatter,
                    "cannot acquire Registry repository `{repository}` because checkout path `{}` already exists",
                    path.display()
                )
            }

            Self::OverlapsInstallation {
                destination,
                installation,
            } => {
                write!(
                    formatter,
                    "source destination `{}` overlaps Vapor Installation `{}`",
                    destination.display(),
                    installation.display()
                )
            }

            Self::UnsupportedProvider {
                repository,
                provider,
            } => {
                write!(
                    formatter,
                    "Registry repository `{repository}` uses unsupported provider `{provider}`"
                )
            }

            Self::UnsupportedRepositoryKind { repository, kind } => {
                write!(
                    formatter,
                    "Registry repository `{repository}` has unsupported top-level kind `{kind}`"
                )
            }

            Self::UnsafeCheckoutPath { repository, path } => {
                write!(
                    formatter,
                    "Registry repository `{repository}` declares unsafe checkout path `{path}`"
                )
            }

            Self::DuplicateCheckoutPath(path) => {
                write!(
                    formatter,
                    "Registry declares duplicate checkout path `{}`",
                    path.display()
                )
            }

            Self::GitStart {
                repository,
                operation,
                source,
            } => {
                write!(
                    formatter,
                    "failed to start Git {operation} for `{repository}`: {source}"
                )
            }

            Self::GitFailed {
                repository,
                operation,
                status,
            } => {
                write!(
                    formatter,
                    "Git {operation} failed for `{repository}` with {status}"
                )
            }

            Self::Superworkspace(error) => error.fmt(formatter),

            Self::UnexpectedSuperworkspace {
                expected,
                discovered,
            } => {
                write!(
                    formatter,
                    "acquired Superworkspace `{}` instead of expected `{}`",
                    discovered.display(),
                    expected.display()
                )
            }

            Self::Source(error) => error.fmt(formatter),

            Self::Io { path, source } => {
                write!(formatter, "failed to access `{}`: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for EcosystemError {}
