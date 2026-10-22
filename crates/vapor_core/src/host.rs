//! Host target identity and runtime environment capabilities.
//!
//! A compiler target answers a different question from the runtime environment.
//! Keep the two separate: `x86_64-unknown-linux-gnu` does not imply X11,
//! Wayland, a GPU API, or any other runtime facility.

use std::env;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostOperatingSystem {
    Linux,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostArchitecture {
    X86_64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostAbi {
    Gnu,
    Msvc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostTarget {
    pub operating_system: HostOperatingSystem,
    pub architecture: HostArchitecture,
    pub abi: HostAbi,
}

impl HostTarget {
    pub const LINUX_X86_64_GNU: Self = Self {
        operating_system: HostOperatingSystem::Linux,
        architecture: HostArchitecture::X86_64,
        abi: HostAbi::Gnu,
    };

    pub const WINDOWS_X86_64_MSVC: Self = Self {
        operating_system: HostOperatingSystem::Windows,
        architecture: HostArchitecture::X86_64,
        abi: HostAbi::Msvc,
    };

    pub const WINDOWS_X86_64_GNULLVM: Self = Self {
        operating_system: HostOperatingSystem::Windows,
        architecture: HostArchitecture::X86_64,
        abi: HostAbi::Gnu,
    };

    pub fn current() -> Result<Self, HostEnvironmentError> {
        let abi = if cfg!(target_env = "gnu") {
            "gnu"
        } else if cfg!(target_env = "msvc") {
            "msvc"
        } else {
            "unsupported"
        };
        Self::from_target_components(env::consts::ARCH, env::consts::OS, abi)
    }

    fn from_target_components(
        architecture: &str,
        operating_system: &str,
        abi: &str,
    ) -> Result<Self, HostEnvironmentError> {
        match (architecture, operating_system, abi) {
            ("x86_64", "linux", "gnu") => Ok(Self::LINUX_X86_64_GNU),
            ("x86_64", "windows", "msvc") => Ok(Self::WINDOWS_X86_64_MSVC),
            ("x86_64", "windows", "gnu") => Ok(Self::WINDOWS_X86_64_GNULLVM),
            _ => Err(HostEnvironmentError::UnsupportedHost),
        }
    }

    pub const fn rustup_host_triple(self) -> Option<&'static str> {
        match (self.operating_system, self.architecture, self.abi) {
            (HostOperatingSystem::Linux, HostArchitecture::X86_64, HostAbi::Gnu) => {
                Some("x86_64-unknown-linux-gnu")
            }
            (HostOperatingSystem::Windows, HostArchitecture::X86_64, HostAbi::Msvc)
            | (HostOperatingSystem::Windows, HostArchitecture::X86_64, HostAbi::Gnu) => {
                Some("x86_64-pc-windows-msvc")
            }
            _ => None,
        }
    }

    pub const fn triple(self) -> &'static str {
        match (self.operating_system, self.architecture, self.abi) {
            (HostOperatingSystem::Linux, HostArchitecture::X86_64, HostAbi::Gnu) => {
                "x86_64-unknown-linux-gnu"
            }
            (HostOperatingSystem::Windows, HostArchitecture::X86_64, HostAbi::Msvc) => {
                "x86_64-pc-windows-msvc"
            }
            (HostOperatingSystem::Windows, HostArchitecture::X86_64, HostAbi::Gnu) => {
                "x86_64-pc-windows-gnullvm"
            }
            _ => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DisplayServer {
    X11,
    Wayland,
}

impl DisplayServer {
    pub const fn key(self) -> &'static str {
        match self {
            Self::X11 => "x11",
            Self::Wayland => "wayland",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostCapability {
    Display(DisplayServer),
}

impl HostCapability {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Display(DisplayServer::X11) => "display-x11",
            Self::Display(DisplayServer::Wayland) => "display-wayland",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEnvironment {
    pub target: HostTarget,
    capabilities: Vec<HostCapability>,
}

impl HostEnvironment {
    pub fn current() -> Result<Self, HostEnvironmentError> {
        let target = HostTarget::current()?;
        let mut capabilities = Vec::new();

        if target.operating_system == HostOperatingSystem::Linux
            && let Some(display) = detect_linux_display_server()
        {
            capabilities.push(HostCapability::Display(display));
        }

        Ok(Self {
            target,
            capabilities,
        })
    }

    pub fn capabilities(&self) -> &[HostCapability] {
        &self.capabilities
    }

    pub fn has(&self, capability: HostCapability) -> bool {
        self.capabilities.contains(&capability)
    }

    pub fn display_server(&self) -> Option<DisplayServer> {
        self.capabilities
            .iter()
            .map(|capability| match capability {
                HostCapability::Display(display) => *display,
            })
            .next()
    }
}

fn detect_linux_display_server() -> Option<DisplayServer> {
    if let Some(session_type) = env::var_os("XDG_SESSION_TYPE")
        .filter(|value| !value.is_empty())
        .and_then(|value| value.into_string().ok())
    {
        if session_type.eq_ignore_ascii_case("wayland") {
            return Some(DisplayServer::Wayland);
        }
        if session_type.eq_ignore_ascii_case("x11") {
            return Some(DisplayServer::X11);
        }
    }

    if env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty()) {
        return Some(DisplayServer::Wayland);
    }

    if env::var_os("DISPLAY").is_some_and(|value| !value.is_empty()) {
        return Some(DisplayServer::X11);
    }

    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEnvironmentError {
    UnsupportedHost,
}

impl fmt::Display for HostEnvironmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedHost => write!(
                formatter,
                "this host target is not yet represented by Vapor's host model"
            ),
        }
    }
}

impl std::error::Error for HostEnvironmentError {}

#[cfg(test)]
mod tests {
    use super::{HostEnvironmentError, HostTarget};

    #[test]
    fn windows_gnullvm_app_maps_to_a_windows_runtime_target_and_rustup_host() {
        let target = HostTarget::from_target_components("x86_64", "windows", "gnu")
            .expect("the Windows GNU-linked Vapor app is a supported runtime target");

        assert_eq!(target.triple(), "x86_64-pc-windows-gnullvm");
        assert_eq!(target.rustup_host_triple(), Some("x86_64-pc-windows-msvc"));
    }

    #[test]
    fn unknown_host_combinations_remain_rejected() {
        assert_eq!(
            HostTarget::from_target_components("aarch64", "windows", "gnu"),
            Err(HostEnvironmentError::UnsupportedHost)
        );
    }
}
