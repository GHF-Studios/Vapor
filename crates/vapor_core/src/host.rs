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

    pub fn current() -> Result<Self, HostEnvironmentError> {
        if cfg!(all(
            target_arch = "x86_64",
            target_os = "linux",
            target_env = "gnu"
        )) {
            Ok(Self::LINUX_X86_64_GNU)
        } else if cfg!(all(
            target_arch = "x86_64",
            target_os = "windows",
            target_env = "msvc"
        )) {
            Ok(Self::WINDOWS_X86_64_MSVC)
        } else {
            Err(HostEnvironmentError::UnsupportedHost)
        }
    }

    pub const fn triple(self) -> &'static str {
        match (
            self.operating_system,
            self.architecture,
            self.abi,
        ) {
            (
                HostOperatingSystem::Linux,
                HostArchitecture::X86_64,
                HostAbi::Gnu,
            ) => "x86_64-unknown-linux-gnu",
            (
                HostOperatingSystem::Windows,
                HostArchitecture::X86_64,
                HostAbi::Msvc,
            ) => "x86_64-pc-windows-msvc",
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

        if target.operating_system == HostOperatingSystem::Linux {
            if let Some(display) = detect_linux_display_server() {
                capabilities.push(HostCapability::Display(display));
            }
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
        self.capabilities.iter().find_map(|capability| match capability {
            HostCapability::Display(display) => Some(*display),
        })
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
