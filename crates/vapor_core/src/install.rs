//! Vapor Installation bootstrap and machine integration.
//!
//! This is the ordinary post-acquisition setup path. It establishes persistent
//! local state and global command discovery without installing developer-role
//! tooling or authored source.

use crate::{VAPOR_HOME_ENV, VaporInstallation};
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const PROFILE_BLOCK_START: &str = "# >>> Vapor managed PATH >>>";
const PROFILE_BLOCK_END: &str = "# <<< Vapor managed PATH <<<";

#[derive(Debug, Clone)]
pub struct InstallReport {
    pub installation_root: PathBuf,
    pub user_data_root: PathBuf,
    pub integration: Vec<String>,
}

pub fn install_installation() -> Result<InstallReport, InstallError> {
    let installation = VaporInstallation::discover().map_err(InstallError::Installation)?;
    let user_data_root = installation.user_data_root();

    installation
        .ensure_state_root()
        .map_err(InstallError::Installation)?;

    let integration = install_machine_integration(&installation)?;

    Ok(InstallReport {
        installation_root: installation.root,
        user_data_root,
        integration,
    })
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
            changed.push(format!("updated managed Vapor PATH block in {}", path.display()));
        }
    }

    Ok(changed)
}

fn host_target_triple() -> Result<&'static str, InstallError> {
    if cfg!(all(
        target_arch = "x86_64",
        target_os = "linux",
        target_env = "gnu"
    )) {
        Ok("x86_64-unknown-linux-gnu")
    } else if cfg!(all(
        target_arch = "x86_64",
        target_os = "windows",
        target_env = "msvc"
    )) {
        Ok("x86_64-pc-windows-msvc")
    } else {
        Err(InstallError::UnsupportedHost)
    }
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

    vec![(profile, true), (bashrc.clone(), bashrc.is_file()), (zshrc.clone(), zshrc.is_file())]
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
$root = $args[0]
$bin = $args[1]
[Environment]::SetEnvironmentVariable('VAPOR_HOME', $root, 'User')
$path = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($null -eq $path) { $path = '' }
$entries = @($path -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
$present = $entries | Where-Object { [string]::Equals($_, $bin, [StringComparison]::OrdinalIgnoreCase) }
if (-not $present) { $entries += $bin }
[Environment]::SetEnvironmentVariable('Path', ($entries -join ';'), 'User')
"#;

    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .arg(root)
        .arg(bin)
        .status()
        .map_err(|source| InstallError::CommandStart {
            command: "PowerShell user environment setup".to_owned(),
            source,
        })?;

    if !status.success() {
        return Err(InstallError::CommandFailed {
            command: "PowerShell user environment setup".to_owned(),
            status: status.to_string(),
        });
    }

    Ok(())
}

#[derive(Debug)]
pub enum InstallError {
    Installation(crate::InstallationError),
    UnsupportedHost,
    MissingBinaryDirectory { path: PathBuf },
    MalformedProfileBlock { path: PathBuf },
    CommandStart { command: String, source: io::Error },
    CommandFailed { command: String, status: String },
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installation(error) => error.fmt(formatter),
            Self::UnsupportedHost => formatter.write_str(
                "this host is not yet supported by Vapor Installation integration",
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
            Self::CommandStart { source, .. } | Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
