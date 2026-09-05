//! Vapor installed-role model.
//!
//! Vapor roles model progressively installed local capability.
//!
//! They are deliberately distinct from external authorization and from USF
//! Capabilities.

use crate::{ManagedToolchain, ToolchainError, VaporInstallation};
use serde::{Deserialize, Serialize};
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::str::FromStr;

const ROLE_STATE_SCHEMA: u32 = 1;
const ROLE_STATE_FILE_NAME: &str = "role.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VaporRole {
    Player,
    Composer,
    ContentDeveloper,
    EcosystemDeveloper,
    RootAuthority,
}

impl VaporRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Player => "player",
            Self::Composer => "composer",
            Self::ContentDeveloper => "content-developer",
            Self::EcosystemDeveloper => "ecosystem-developer",
            Self::RootAuthority => "root-authority",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Player => "Player",
            Self::Composer => "Composer",
            Self::ContentDeveloper => "Content Developer",
            Self::EcosystemDeveloper => "Ecosystem Developer",
            Self::RootAuthority => "Root Authority",
        }
    }
}

impl fmt::Display for VaporRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.display_name())
    }
}

impl FromStr for VaporRole {
    type Err = ParseVaporRoleError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "player" => Ok(Self::Player),
            "composer" => Ok(Self::Composer),
            "content-developer" => Ok(Self::ContentDeveloper),
            "ecosystem-developer" => Ok(Self::EcosystemDeveloper),
            "root-authority" => Ok(Self::RootAuthority),

            _ => Err(ParseVaporRoleError {
                value: value.to_owned(),
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoleStatus {
    pub installed_role: VaporRole,
    pub state_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct RoleTransitionReport {
    pub previous_role: VaporRole,
    pub installed_role: VaporRole,
    pub toolchain_installed: bool,
}

pub fn role_status(installation: &VaporInstallation) -> Result<RoleStatus, RoleError> {
    Ok(RoleStatus {
        installed_role: installed_role(installation)?,
        state_path: role_state_path(installation),
    })
}

pub fn installed_role(installation: &VaporInstallation) -> Result<VaporRole, RoleError> {
    let path = role_state_path(installation);

    if !path.exists() {
        return Ok(VaporRole::Player);
    }

    let source = fs::read_to_string(&path).map_err(|source| RoleError::Io {
        path: path.clone(),
        source,
    })?;

    let state: PersistedRoleState = toml::from_str(&source).map_err(|error| RoleError::State {
        path: path.clone(),
        message: error.to_string(),
    })?;

    if state.schema != ROLE_STATE_SCHEMA {
        return Err(RoleError::UnsupportedSchema {
            path,
            found: state.schema,
            supported: ROLE_STATE_SCHEMA,
        });
    }

    Ok(state.role)
}

/// Promote the active Vapor Installation.
///
/// The current rewrite bootstrap can locally establish roles through Content
/// Developer. Ecosystem Developer and Root Authority additionally require
/// external official authorization and therefore cannot yet be established by
/// this local operation.
///
/// Content Developer promotion currently obtains its pinned toolchain from the
/// enclosing Vapor source Workspace. Once the real Steam installation manifest
/// becomes authoritative, the pin will instead come from that Installation.
pub fn promote_role(
    installation: &VaporInstallation,
    target: VaporRole,
) -> Result<RoleTransitionReport, RoleError> {
    let current = installed_role(installation)?;

    if target <= current {
        return Err(RoleError::InvalidPromotion {
            current,
            requested: target,
        });
    }

    if target > VaporRole::ContentDeveloper {
        return Err(RoleError::PrivilegedPromotion { requested: target });
    }

    if target >= VaporRole::Composer && !git_available() {
        return Err(RoleError::GitUnavailable);
    }

    let mut toolchain_installed = false;

    if target >= VaporRole::ContentDeveloper {
        let toolchain = ManagedToolchain::discover().map_err(RoleError::Toolchain)?;

        if !toolchain.is_installed() {
            toolchain.install().map_err(RoleError::Toolchain)?;
            toolchain_installed = true;
        }
    }

    persist_role(installation, target)?;

    Ok(RoleTransitionReport {
        previous_role: current,
        installed_role: target,
        toolchain_installed,
    })
}

/// Lower the locally installed Vapor role.
///
/// This operation is intentionally non-destructive. It changes installed role
/// state but does not yet remove toolchains, caches, or authored source.
///
/// Explicit capability cleanup belongs to a later Installer operation.
pub fn demote_role(
    installation: &VaporInstallation,
    target: VaporRole,
) -> Result<RoleTransitionReport, RoleError> {
    let current = installed_role(installation)?;

    if target >= current {
        return Err(RoleError::InvalidDemotion {
            current,
            requested: target,
        });
    }

    persist_role(installation, target)?;

    Ok(RoleTransitionReport {
        previous_role: current,
        installed_role: target,
        toolchain_installed: false,
    })
}

pub fn git_available() -> bool {
    let Some(path) = env::var_os("PATH") else {
        return false;
    };

    let executable = executable_name("git");

    env::split_paths(&path)
        .map(|root| root.join(&executable))
        .any(|candidate| candidate.is_file())
}

fn persist_role(installation: &VaporInstallation, role: VaporRole) -> Result<(), RoleError> {
    let state_root = installation
        .ensure_state_root()
        .map_err(RoleError::Installation)?;

    let path = state_root.join(ROLE_STATE_FILE_NAME);

    let state = PersistedRoleState {
        schema: ROLE_STATE_SCHEMA,
        role,
    };

    let source = toml::to_string_pretty(&state).map_err(|error| RoleError::State {
        path: path.clone(),
        message: error.to_string(),
    })?;

    fs::write(&path, source).map_err(|source| RoleError::Io { path, source })
}

fn role_state_path(installation: &VaporInstallation) -> PathBuf {
    installation.state_root().join(ROLE_STATE_FILE_NAME)
}

fn executable_name(stem: &str) -> String {
    format!("{stem}{}", env::consts::EXE_SUFFIX)
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedRoleState {
    schema: u32,
    role: VaporRole,
}

#[derive(Debug, Clone)]
pub struct ParseVaporRoleError {
    value: String,
}

impl fmt::Display for ParseVaporRoleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unknown Vapor role `{}`; expected player, composer, content-developer, ecosystem-developer, or root-authority",
            self.value
        )
    }
}

impl std::error::Error for ParseVaporRoleError {}

#[derive(Debug)]
pub enum RoleError {
    Installation(crate::InstallationError),

    Toolchain(ToolchainError),

    Io {
        path: PathBuf,
        source: io::Error,
    },

    State {
        path: PathBuf,
        message: String,
    },

    UnsupportedSchema {
        path: PathBuf,
        found: u32,
        supported: u32,
    },

    InvalidPromotion {
        current: VaporRole,
        requested: VaporRole,
    },

    InvalidDemotion {
        current: VaporRole,
        requested: VaporRole,
    },

    PrivilegedPromotion {
        requested: VaporRole,
    },

    GitUnavailable,
}

impl fmt::Display for RoleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installation(error) => error.fmt(formatter),

            Self::Toolchain(error) => error.fmt(formatter),

            Self::Io { path, source } => {
                write!(
                    formatter,
                    "failed to access Vapor role state `{}`: {source}",
                    path.display()
                )
            }

            Self::State { path, message } => {
                write!(
                    formatter,
                    "invalid Vapor role state `{}`: {message}",
                    path.display()
                )
            }

            Self::UnsupportedSchema {
                path,
                found,
                supported,
            } => {
                write!(
                    formatter,
                    "unsupported Vapor role-state schema {found} in `{}`; this Vapor supports schema {supported}",
                    path.display()
                )
            }

            Self::InvalidPromotion { current, requested } => {
                write!(
                    formatter,
                    "cannot promote from {current} to {requested}; promotion must move to a higher role"
                )
            }

            Self::InvalidDemotion { current, requested } => {
                write!(
                    formatter,
                    "cannot demote from {current} to {requested}; demotion must move to a lower role"
                )
            }

            Self::PrivilegedPromotion { requested } => {
                write!(
                    formatter,
                    "{requested} requires official external authorization and cannot yet be established by local Installer promotion"
                )
            }

            Self::GitUnavailable => {
                write!(
                    formatter,
                    "Composer-or-higher capability requires Git, but no Git executable is available on PATH"
                )
            }
        }
    }
}

impl std::error::Error for RoleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Installation(error) => Some(error),
            Self::Toolchain(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
