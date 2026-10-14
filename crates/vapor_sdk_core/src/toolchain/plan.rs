//! Rustup invocation planning for the canonical Vapor toolchain.

use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use super::{ToolchainComponent, ToolchainStatus, ToolchainStatusError, toolchain_status};

/// Zero-mutation plan for installing the canonical Vapor Rust/Cargo toolchain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolchainInstallPlan {
    pub status: ToolchainStatus,
    pub rustup_path: PathBuf,
    pub rustup_args: Vec<String>,
    pub components: Vec<String>,
    pub targets: Vec<String>,
}

/// Build the Rustup command line without mutating local state.
pub fn toolchain_install_plan() -> Result<ToolchainInstallPlan, ToolchainPlanError> {
    let status = toolchain_status()?;

    if !status.host_supported {
        return Err(ToolchainPlanError::UnsupportedHost(
            status.host_triple.to_owned(),
        ));
    }

    let Some(rustup_path) = status.rustup_path.clone() else {
        return Err(ToolchainPlanError::RustupUnavailable {
            expected: status.local_rustup_path.clone(),
        });
    };

    let components = rustup_components(&status);
    let targets = status
        .supported_target_triples()
        .iter()
        .map(|target| (*target).to_owned())
        .collect::<Vec<_>>();
    let rustup_args = rustup_toolchain_install_args(&status, &components, &targets);

    Ok(ToolchainInstallPlan {
        status,
        rustup_path,
        rustup_args,
        components,
        targets,
    })
}

fn rustup_components(status: &ToolchainStatus) -> Vec<String> {
    status
        .toolchain
        .required_components()
        .iter()
        .filter_map(|component| match component {
            ToolchainComponent::Rustfmt => Some("rustfmt"),
            ToolchainComponent::Clippy => Some("clippy"),
            ToolchainComponent::RustSrc => Some("rust-src"),
            ToolchainComponent::Rustc | ToolchainComponent::Cargo | ToolchainComponent::RustStd => {
                None
            }
        })
        .map(str::to_owned)
        .collect()
}

fn rustup_toolchain_install_args(
    status: &ToolchainStatus,
    components: &[String],
    targets: &[String],
) -> Vec<String> {
    let mut args = vec![
        "toolchain".to_owned(),
        "install".to_owned(),
        status.toolchain.identifier().to_owned(),
        "--profile".to_owned(),
        "minimal".to_owned(),
        "--no-self-update".to_owned(),
    ];

    for component in components {
        args.push("--component".to_owned());
        args.push(component.clone());
    }

    for target in targets {
        args.push("--target".to_owned());
        args.push(target.clone());
    }

    args
}

#[derive(Debug)]
pub enum ToolchainPlanError {
    Status(ToolchainStatusError),
    UnsupportedHost(String),
    RustupUnavailable { expected: PathBuf },
}

impl From<ToolchainStatusError> for ToolchainPlanError {
    fn from(error: ToolchainStatusError) -> Self {
        Self::Status(error)
    }
}

impl fmt::Display for ToolchainPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status(error) => write!(formatter, "{error}"),
            Self::UnsupportedHost(host) => write!(
                formatter,
                "host triple `{host}` is not supported by this Vapor toolchain pin"
            ),
            Self::RustupUnavailable { expected } => write!(
                formatter,
                "Rustup is not available; install or place it at `{}`",
                expected.display()
            ),
        }
    }
}

impl Error for ToolchainPlanError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Status(error) => Some(error),
            Self::UnsupportedHost(_) | Self::RustupUnavailable { .. } => None,
        }
    }
}
