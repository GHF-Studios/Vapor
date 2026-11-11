//! Vapor Installation bootstrap and machine integration.
//!
//! This is the ordinary post-acquisition setup path. It establishes persistent
//! local state and global command discovery without installing developer-role
//! tooling or authored source.

use crate::{
    SourceError, SuperworkspaceError, VAPOR_HOME_ENV, VaporInstallation, VaporSuperworkspace,
    configure_superworkspace, source_state,
};
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const PROFILE_BLOCK_START: &str = "# >>> Vapor managed PATH >>>";
const PROFILE_BLOCK_END: &str = "# <<< Vapor managed PATH <<<";

#[derive(Debug, Clone)]
pub struct InstallReport {
    pub installation_root: PathBuf,
    pub data_root: PathBuf,
    pub superworkspace_root: PathBuf,
    pub integration: Vec<String>,
}

pub fn install_installation() -> Result<InstallReport, InstallError> {
    let installation = VaporInstallation::discover().map_err(InstallError::Installation)?;
    install_installation_for(&installation)
}

/// Install Vapor's machine integration for a specific app root.
pub fn install_installation_for(
    installation: &VaporInstallation,
) -> Result<InstallReport, InstallError> {
    let data_root = installation.data_root();

    installation
        .ensure_state_root()
        .map_err(InstallError::Installation)?;

    let superworkspace = ensure_canonical_superworkspace(installation)?;
    let integration = install_machine_integration(installation)?;

    Ok(InstallReport {
        installation_root: installation.root.clone(),
        data_root,
        superworkspace_root: superworkspace.root,
        integration,
    })
}

pub fn ensure_canonical_superworkspace(
    installation: &VaporInstallation,
) -> Result<VaporSuperworkspace, InstallError> {
    let state = source_state(installation).map_err(InstallError::Source)?;
    let root = state
        .superworkspace
        .unwrap_or_else(|| installation.root.clone());

    configure_superworkspace(installation, &root).map_err(InstallError::Source)
}

fn install_machine_integration(
    installation: &VaporInstallation,
) -> Result<Vec<String>, InstallError> {
    let target = host_target_triple()?;
    let bin = installation.root.join("bin").join(target);

    if !bin.is_dir() {
        return Err(InstallError::MissingBinaryDirectory { path: bin });
    }

    if cfg!(windows) {
        install_windows_user_integration(&installation.root, &bin)?;
        return Ok(vec![format!(
            "installed user {VAPOR_HOME_ENV} and PATH integration for {}",
            bin.display()
        )]);
    }

    let block = unix_profile_block(&installation.root, target);
    let mut changed = Vec::new();

    for (path, create) in shell_profiles() {
        if upsert_profile_block(&path, &block, create)? {
            changed.push(format!(
                "updated managed Vapor PATH block in {}",
                path.display()
            ));
        }
    }

    Ok(changed)
}

fn host_target_triple() -> Result<&'static str, InstallError> {
    crate::HostTarget::current()
        .map(|target| target.triple())
        .map_err(|_| InstallError::UnsupportedHost)
}

fn unix_profile_block(installation_root: &Path, target: &str) -> String {
    let root = shell_single_quote(&installation_root.to_string_lossy());

    format!(
        "{PROFILE_BLOCK_START}\nexport {VAPOR_HOME_ENV}={root}\nexport PATH=\"${VAPOR_HOME_ENV}/bin/{target}:$PATH\"\n{PROFILE_BLOCK_END}\n"
    )
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn shell_profiles() -> Vec<(PathBuf, bool)> {
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };

    let profile = home.join(".profile");
    let bashrc = home.join(".bashrc");
    let zshrc = home.join(".zshrc");

    vec![
        (profile, true),
        (bashrc.clone(), bashrc.is_file()),
        (zshrc.clone(), zshrc.is_file()),
    ]
}

fn upsert_profile_block(path: &Path, block: &str, create: bool) -> Result<bool, InstallError> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound && create => String::new(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(InstallError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let without = remove_profile_block(&source, path)?;
    let mut output = without.trim_end_matches('\n').to_owned();

    if !output.is_empty() {
        output.push_str("\n\n");
    }

    output.push_str(block);

    if output == source {
        return Ok(false);
    }

    fs::write(path, output).map_err(|source| InstallError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    Ok(true)
}

fn remove_profile_block(source: &str, path: &Path) -> Result<String, InstallError> {
    let mut inside = false;
    let mut output = String::new();

    for line in source.lines() {
        if line == PROFILE_BLOCK_START {
            if inside {
                return Err(InstallError::MalformedProfileBlock {
                    path: path.to_path_buf(),
                });
            }
            inside = true;
            continue;
        }

        if inside {
            if line == PROFILE_BLOCK_END {
                inside = false;
            }
            continue;
        }

        output.push_str(line);
        output.push('\n');
    }

    if inside {
        return Err(InstallError::MalformedProfileBlock {
            path: path.to_path_buf(),
        });
    }

    Ok(output)
}

fn install_windows_user_integration(root: &Path, bin: &Path) -> Result<(), InstallError> {
    let script = r#"
param([string]$root, [string]$bin)
$ErrorActionPreference = 'Stop'
[Environment]::SetEnvironmentVariable('VAPOR_HOME', $root, 'User')
$path = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($null -eq $path) { $path = '' }
$entries = @($path -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
$canonicalBin = [IO.Path]::GetFullPath($bin).TrimEnd([char[]]@([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar))
$entries = @($entries | Where-Object {
    try {
        $candidate = $_
        if ($candidate.StartsWith('\\?\UNC\', [StringComparison]::OrdinalIgnoreCase)) {
            $candidate = '\\' + $candidate.Substring(8)
        } elseif ($candidate.StartsWith('\\?\', [StringComparison]::OrdinalIgnoreCase)) {
            $candidate = $candidate.Substring(4)
        }
        $entry = [IO.Path]::GetFullPath($candidate).TrimEnd([char[]]@([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar))
        -not [string]::Equals($entry, $canonicalBin, [StringComparison]::OrdinalIgnoreCase)
    } catch { $true }
})
$entries += $bin
[Environment]::SetEnvironmentVariable('Path', ($entries -join ';'), 'User')
$storedHome = [Environment]::GetEnvironmentVariable('VAPOR_HOME', 'User')
$storedPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$hasBin = @($storedPath -split ';' | Where-Object {
    try {
        $entry = [IO.Path]::GetFullPath($_).TrimEnd([char[]]@([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar))
        [string]::Equals($entry, $canonicalBin, [StringComparison]::OrdinalIgnoreCase)
    } catch { $false }
}).Count -gt 0
if (-not [string]::Equals([IO.Path]::GetFullPath($storedHome), [IO.Path]::GetFullPath($root), [StringComparison]::OrdinalIgnoreCase) -or -not $hasBin) {
    throw 'Windows user environment did not retain VAPOR_HOME and the Vapor command directory.'
}
try {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class VaporEnvironmentBroadcast {
    [DllImport("user32.dll", CharSet = CharSet.Auto)]
    public static extern IntPtr SendMessageTimeout(IntPtr hWnd, UInt32 message, IntPtr wParam, string lParam, UInt32 flags, UInt32 timeout, out IntPtr result);
}
'@
    $result = [IntPtr]::Zero
    $sent = [VaporEnvironmentBroadcast]::SendMessageTimeout([IntPtr](-1), 0x001A, [IntPtr]::Zero, 'Environment', 0x0002, 5000, [ref]$result)
    if ($sent -eq [IntPtr]::Zero) { Write-Warning 'PATH was saved, but Windows did not notify open applications.' }
} catch {
    Write-Warning "The user PATH was saved, but Windows could not broadcast the environment change: $_"
}
Write-Output 'registered'
"#;

    let command = format!(
        "& {{ {script} }} {} {}",
        powershell_literal(&windows_environment_path(root)),
        powershell_literal(&windows_environment_path(bin)),
    );
    let output = crate::managed_process::background_command(
        crate::managed_process::windows_powershell_executable(),
    )
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(command)
        .output()
        .map_err(|source| InstallError::CommandStart {
            command: "PowerShell user environment setup".to_owned(),
            source,
        })?;

    if !output.status.success() {
        return Err(InstallError::CommandFailed {
            command: "PowerShell user environment setup".to_owned(),
            status: format!(
                "{}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }

    Ok(())
}

fn powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

pub(crate) fn windows_environment_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    if let Some(unc_path) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc_path}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(&path).to_owned()
    }
}

#[derive(Debug)]
pub enum InstallError {
    Installation(crate::InstallationError),
    Source(SourceError),
    Superworkspace(SuperworkspaceError),
    UnsupportedHost,
    HomeUnavailable,
    UnsafeSuperworkspaceRoot {
        path: PathBuf,
        installation: PathBuf,
        user_data: PathBuf,
    },
    MissingBinaryDirectory {
        path: PathBuf,
    },
    MalformedProfileBlock {
        path: PathBuf,
    },
    CommandStart {
        command: String,
        source: io::Error,
    },
    CommandFailed {
        command: String,
        status: String,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installation(error) => error.fmt(formatter),
            Self::Source(error) => error.fmt(formatter),
            Self::Superworkspace(error) => error.fmt(formatter),
            Self::UnsupportedHost => formatter
                .write_str("this host is not yet supported by Vapor Installation integration"),
            Self::HomeUnavailable => {
                formatter.write_str("cannot determine the current user's home directory")
            }
            Self::UnsafeSuperworkspaceRoot {
                path,
                installation,
                user_data,
            } => write!(
                formatter,
                "canonical Superworkspace `{}` must not overlap Vapor Installation `{}` or Vapor User Data `{}`",
                path.display(),
                installation.display(),
                user_data.display(),
            ),
            Self::MissingBinaryDirectory { path } => write!(
                formatter,
                "Vapor Installation is missing its host binary directory `{}`",
                path.display()
            ),
            Self::MalformedProfileBlock { path } => write!(
                formatter,
                "managed Vapor PATH block in `{}` has no closing marker",
                path.display()
            ),
            Self::CommandStart { command, source } => {
                write!(formatter, "failed to start {command}: {source}")
            }
            Self::CommandFailed { command, status } => {
                write!(formatter, "{command} failed with {status}")
            }
            Self::Io { path, source } => {
                write!(formatter, "failed to modify `{}`: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for InstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Installation(error) => Some(error),
            Self::Source(error) => Some(error),
            Self::Superworkspace(error) => Some(error),
            Self::CommandStart { source, .. } | Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
