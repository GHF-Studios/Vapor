//! Vapor installed-role model.
//!
//! A Vapor Role describes the kinds of work for which the local Vapor
//! Installation is equipped.
//!
//! Role is deliberately distinct from external authorization and from USF
//! Capabilities.

use crate::{DeveloperEnvironment, DeveloperEnvironmentError, VaporInstallation};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::str::FromStr;

const ROLE_STATE_SCHEMA: u32 = 2;
const ROLE_STATE_FILE_NAME: &str = "role.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VaporRole {
    Player,
    Developer,
}

impl VaporRole {
    /// Whether this role promises the complete locally managed authoring and
    /// development toolset is installed.
    ///
    /// Keep this policy centralized so promotion, diagnosis, and repair cannot
    /// disagree about which roles own developer tools.
    pub const fn requires_developer_environment(self) -> bool {
        matches!(
            self,
            Self::Developer
        )
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Player => "player",
            Self::Developer => "developer",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Player => "Player",
            Self::Developer => "Developer",
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
            "developer" => Ok(Self::Developer),

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
    pub developer_environment_changed: bool,
}

pub fn role_status(installation: &VaporInstallation) -> Result<RoleStatus, RoleError> {
    Ok(RoleStatus {
        installed_role: installed_role(installation)?,
        state_path: role_state_path(installation),
    })
}

pub fn installed_role(installation: &VaporInstallation) -> Result<VaporRole, RoleError> {
    let path = installation
        .ensure_state_root()
        .map_err(RoleError::Installation)?
        .join(ROLE_STATE_FILE_NAME);

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

    decode_persisted_role(state).map_err(|error| match error {
        PersistedRoleError::UnsupportedSchema(found) => RoleError::UnsupportedSchema {
            path,
            found,
            supported: ROLE_STATE_SCHEMA,
        },
        PersistedRoleError::UnknownRole { schema, role } => RoleError::State {
            path,
            message: format!("unknown role `{role}` in schema {schema}"),
        },
    })
}

/// Read a persisted role for the legacy-state migration path. Migration uses
/// the same schema interpretation as normal role loading so equivalent old
/// and new records can be reconciled without weakening byte-level conflict
/// handling for other state files.
pub(crate) fn read_persisted_role(path: &std::path::Path) -> Result<VaporRole, String> {
    let source = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let state: PersistedRoleState = toml::from_str(&source).map_err(|error| error.to_string())?;
    decode_persisted_role(state).map_err(|error| error.to_string())
}

fn decode_persisted_role(state: PersistedRoleState) -> Result<VaporRole, PersistedRoleError> {
    match state.schema {
        ROLE_STATE_SCHEMA => match state.role.as_str() {
            "player" => Ok(VaporRole::Player),
            "developer" => Ok(VaporRole::Developer),
            _ => Err(PersistedRoleError::UnknownRole {
                schema: state.schema,
                role: state.role,
            }),
        },
        // Schema 1's three creator roles all owned the complete toolchain;
        // schema 2 consolidates them into the single Developer role.
        1 => match state.role.as_str() {
            "player" => Ok(VaporRole::Player),
            "composer" | "content-developer" | "ecosystem-developer" => {
                Ok(VaporRole::Developer)
            }
            _ => Err(PersistedRoleError::UnknownRole {
                schema: state.schema,
                role: state.role,
            }),
        },
        found => Err(PersistedRoleError::UnsupportedSchema(found)),
    }
}

/// Promote the active Vapor Installation.
///
/// Player and Developer are the only locally attainable setup roles.
///
/// External authorization is evaluated separately when a particular operation
/// targets a protected official resource.
///
/// Developer promotion reconciles tools against the selected Vapor Installation.
/// Source workspaces may provide the toolchain pin during local development,
/// while installed releases use their packaged pin metadata.
pub fn promote_role(
    installation: &VaporInstallation,
    target: VaporRole,
) -> Result<RoleTransitionReport, RoleError> {
    promote_role_inner(installation, target, None)
}

/// Promote the active installation while publishing developer-tool setup to
/// the operation stream consumed by Studio, IDE, Launcher and CLI.
pub fn promote_role_with_context(
    installation: &VaporInstallation,
    target: VaporRole,
    context: &crate::OperationContext,
) -> Result<RoleTransitionReport, RoleError> {
    promote_role_inner(installation, target, Some(context))
}

fn promote_role_inner(
    installation: &VaporInstallation,
    target: VaporRole,
    context: Option<&crate::OperationContext>,
) -> Result<RoleTransitionReport, RoleError> {
    let current = installed_role(installation)?;

    if target <= current {
        return Err(RoleError::InvalidPromotion {
            current,
            requested: target,
        });
    }

    let mut developer_environment_changed = false;

    if target.requires_developer_environment() {
        let mut environment = DeveloperEnvironment::discover_for_installation(installation)
            .map_err(RoleError::DeveloperEnvironment)?;
        let report = match context {
            Some(context) => environment.reconcile_with_context(context),
            None => environment.reconcile(),
        }
        .map_err(RoleError::DeveloperEnvironment)?;
        developer_environment_changed = report.changed();
    }

    persist_role(installation, target)?;

    Ok(RoleTransitionReport {
        previous_role: current,
        installed_role: target,
        developer_environment_changed,
    })
}

/// Lower the locally installed Vapor Role.
///
/// This operation is intentionally non-destructive. It changes installed Role
/// state but does not remove toolchains, caches, or authored source.
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
        developer_environment_changed: false,
    })
}

fn persist_role(installation: &VaporInstallation, role: VaporRole) -> Result<(), RoleError> {
    let state_root = installation
        .ensure_state_root()
        .map_err(RoleError::Installation)?;

    let path = state_root.join(ROLE_STATE_FILE_NAME);

    let state = PersistedRoleState {
        schema: ROLE_STATE_SCHEMA,
        role: role.as_str().to_owned(),
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

#[derive(Debug, Serialize, Deserialize)]
struct PersistedRoleState {
    schema: u32,
    role: String,
}

#[derive(Debug)]
enum PersistedRoleError {
    UnknownRole { schema: u32, role: String },
    UnsupportedSchema(u32),
}

impl fmt::Display for PersistedRoleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRole { schema, role } => {
                write!(formatter, "unknown role `{role}` in schema {schema}")
            }
            Self::UnsupportedSchema(found) => {
                write!(formatter, "unsupported schema {found}; supported schema is {ROLE_STATE_SCHEMA}")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParseVaporRoleError {
    value: String,
}

impl fmt::Display for ParseVaporRoleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "unknown Vapor role `{}`; expected player or developer",
            self.value
        )
    }
}

impl std::error::Error for ParseVaporRoleError {}

#[derive(Debug)]
pub enum RoleError {
    Installation(crate::InstallationError),

    DeveloperEnvironment(DeveloperEnvironmentError),

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
}

impl fmt::Display for RoleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installation(error) => error.fmt(formatter),

            Self::DeveloperEnvironment(error) => error.fmt(formatter),

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
        }
    }
}

impl std::error::Error for RoleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Installation(error) => Some(error),
            Self::DeveloperEnvironment(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::VaporRole;

    #[test]
    fn developer_environment_is_owned_by_the_developer_role() {
        assert!(!VaporRole::Player.requires_developer_environment());
        assert!(VaporRole::Developer.requires_developer_environment());
    }
}
