//! Installation-owned external development tools.
//!
//! Managed tools are disposable local realizations of an explicit tool/version
//! requirement. A discovered PATH executable or user override may satisfy a
//! command, but it does not become Vapor-owned state merely because it was found.

use crate::{HostCapability, HostEnvironment};
use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const GITHUB_API_ACCEPT: &str = "application/vnd.github+json";
const GITHUB_API_VERSION: &str = "2022-11-28";

#[derive(Debug, Clone)]
pub struct ManagedToolRequest {
    pub name: String,
    pub version: String,
    pub executable_names: Vec<String>,
    pub required_capabilities: Vec<HostCapability>,
}

impl ManagedToolRequest {
    pub fn new<I, S, C>(
        name: impl Into<String>,
        version: impl Into<String>,
        executable_names: I,
        required_capabilities: C,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
        C: IntoIterator<Item = HostCapability>,
    {
        Self {
            name: name.into(),
            version: version.into(),
            executable_names: executable_names
                .into_iter()
                .map(Into::into)
                .collect(),
            required_capabilities: required_capabilities.into_iter().collect(),
        }
    }

    pub fn matches_environment(&self, environment: &HostEnvironment) -> bool {
        self.required_capabilities
            .iter()
            .all(|capability| environment.has(*capability))
    }

    pub fn environment_key(&self) -> String {
        if self.required_capabilities.is_empty() {
            return "native".to_owned();
        }

        self.required_capabilities
            .iter()
            .map(|capability| capability.key())
            .collect::<Vec<_>>()
            .join("+")
    }

    pub fn install_root(
        &self,
        user_data_root: &Path,
        environment: &HostEnvironment,
    ) -> PathBuf {
        user_data_root
            .join("tools")
            .join(&self.name)
            .join(&self.version)
            .join(environment.target.triple())
            .join(self.environment_key())
    }

    pub fn installed_executable(&self, root: &Path) -> Option<PathBuf> {
        find_executable(root, &self.executable_names)
    }
}

pub fn find_executable(path: &Path, executable_names: &[String]) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }

    if !path.is_dir() {
        return None;
    }

    for name in executable_names {
        let candidate = path.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

pub fn find_executable_on_path(executable_names: &[String]) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;

    env::split_paths(&path)
        .find_map(|directory| find_executable(&directory, executable_names))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseAssetStatus {
    Downloaded,
    Missing,
}

pub fn github_release_client() -> Result<Client, ManagedToolError> {
    Client::builder()
        .user_agent("Vapor managed external tools")
        .build()
        .map_err(ManagedToolError::Http)
}

/// Download one exact release asset and verify GitHub's SHA-256 digest before
/// promoting it into managed state.
///
/// `Missing` is intentionally distinct from an HTTP/integrity failure so a
/// provider may choose a principled fallback only when the artifact genuinely
/// has not been published.
pub fn download_verified_github_release_asset(
    client: &Client,
    repository: &str,
    tag: &str,
    asset_name: &str,
    destination: &Path,
    executable: bool,
) -> Result<ReleaseAssetStatus, ManagedToolError> {
    let release_url =
        format!("https://api.github.com/repos/{repository}/releases/tags/{tag}");

    let response = client
        .get(&release_url)
        .header(reqwest::header::ACCEPT, GITHUB_API_ACCEPT)
        .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
        .send()
        .map_err(ManagedToolError::Http)?;

    if response.status() == StatusCode::NOT_FOUND {
        return Ok(ReleaseAssetStatus::Missing);
    }

    let response = response.error_for_status().map_err(ManagedToolError::Http)?;
    let body = response.text().map_err(ManagedToolError::Http)?;
    let release: GitHubRelease =
        serde_json::from_str(&body).map_err(ManagedToolError::Json)?;

    let Some(asset) = release.assets.into_iter().find(|asset| asset.name == asset_name) else {
        return Ok(ReleaseAssetStatus::Missing);
    };

    let Some(expected_digest) = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
    else {
        return Err(ManagedToolError::MissingDigest {
            repository: repository.to_owned(),
            tag: tag.to_owned(),
            asset: asset_name.to_owned(),
        });
    };

    let parent = destination
        .parent()
        .ok_or_else(|| ManagedToolError::InvalidDestination(destination.to_path_buf()))?;

    fs::create_dir_all(parent).map_err(|source| ManagedToolError::Io {
        path: parent.to_path_buf(),
        source,
    })?;

    let temporary = parent.join(format!(".{}.download-{}", asset_name, std::process::id()));

    let result = (|| {
        let bytes = client
            .get(&asset.browser_download_url)
            .send()
            .map_err(ManagedToolError::Http)?
            .error_for_status()
            .map_err(ManagedToolError::Http)?
            .bytes()
            .map_err(ManagedToolError::Http)?;

        fs::write(&temporary, &bytes).map_err(|source| ManagedToolError::Io {
            path: temporary.clone(),
            source,
        })?;

        let actual_digest = sha256_file(&temporary)?;
        if !actual_digest.eq_ignore_ascii_case(expected_digest) {
            return Err(ManagedToolError::DigestMismatch {
                asset: asset_name.to_owned(),
                expected: expected_digest.to_owned(),
                actual: actual_digest,
            });
        }

        #[cfg(unix)]
        if executable {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&temporary)
                .map_err(|source| ManagedToolError::Io {
                    path: temporary.clone(),
                    source,
                })?
                .permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&temporary, permissions).map_err(|source| {
                ManagedToolError::Io {
                    path: temporary.clone(),
                    source,
                }
            })?;
        }

        if destination.exists() {
            fs::remove_file(destination).map_err(|source| ManagedToolError::Io {
                path: destination.to_path_buf(),
                source,
            })?;
        }

        fs::rename(&temporary, destination).map_err(|source| ManagedToolError::Io {
            path: destination.to_path_buf(),
            source,
        })?;

        Ok(ReleaseAssetStatus::Downloaded)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }

    result
}

fn sha256_file(path: &Path) -> Result<String, ManagedToolError> {
    #[cfg(target_os = "windows")]
    {
        let output = Command::new("certutil")
            .args(["-hashfile"])
            .arg(path)
            .arg("SHA256")
            .output()
            .map_err(|source| ManagedToolError::DigestTool {
                tool: "certutil",
                source,
            })?;

        if !output.status.success() {
            return Err(ManagedToolError::DigestToolFailed {
                tool: "certutil",
                status: output.status.to_string(),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        return stdout
            .lines()
            .map(str::trim)
            .find(|line| {
                line.len() == 64 && line.chars().all(|character| character.is_ascii_hexdigit())
            })
            .map(str::to_owned)
            .ok_or(ManagedToolError::DigestOutput {
                tool: "certutil",
            });
    }

    #[cfg(not(target_os = "windows"))]
    {
        let output = Command::new("sha256sum")
            .arg(path)
            .output()
            .map_err(|source| ManagedToolError::DigestTool {
                tool: "sha256sum",
                source,
            })?;

        if !output.status.success() {
            return Err(ManagedToolError::DigestToolFailed {
                tool: "sha256sum",
                status: output.status.to_string(),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        return stdout
            .split_whitespace()
            .next()
            .filter(|digest| {
                digest.len() == 64
                    && digest.chars().all(|character| character.is_ascii_hexdigit())
            })
            .map(str::to_owned)
            .ok_or(ManagedToolError::DigestOutput {
                tool: "sha256sum",
            });
    }
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    assets: Vec<GitHubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubReleaseAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

#[derive(Debug)]
pub enum ManagedToolError {
    Http(reqwest::Error),
    Json(serde_json::Error),
    Io {
        path: PathBuf,
        source: io::Error,
    },
    InvalidDestination(PathBuf),
    MissingDigest {
        repository: String,
        tag: String,
        asset: String,
    },
    DigestTool {
        tool: &'static str,
        source: io::Error,
    },
    DigestToolFailed {
        tool: &'static str,
        status: String,
    },
    DigestOutput {
        tool: &'static str,
    },
    DigestMismatch {
        asset: String,
        expected: String,
        actual: String,
    },
}

impl fmt::Display for ManagedToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(error) => write!(formatter, "managed-tool HTTP request failed: {error}"),
            Self::Json(error) => write!(
                formatter,
                "managed-tool GitHub release metadata was invalid JSON: {error}"
            ),
            Self::Io { path, source } => {
                write!(formatter, "failed to access `{}`: {source}", path.display())
            }
            Self::InvalidDestination(path) => {
                write!(formatter, "managed-tool destination `{}` has no parent", path.display())
            }
            Self::MissingDigest {
                repository,
                tag,
                asset,
            } => write!(
                formatter,
                "release asset `{repository}` / `{tag}` / `{asset}` has no SHA-256 digest"
            ),
            Self::DigestTool { tool, source } => {
                write!(formatter, "failed to start `{tool}` for artifact verification: {source}")
            }
            Self::DigestToolFailed { tool, status } => {
                write!(formatter, "`{tool}` failed while verifying an artifact ({status})")
            }
            Self::DigestOutput { tool } => {
                write!(formatter, "`{tool}` returned no recognizable SHA-256 digest")
            }
            Self::DigestMismatch {
                asset,
                expected,
                actual,
            } => write!(
                formatter,
                "SHA-256 mismatch for `{asset}`: expected {expected}, got {actual}"
            ),
        }
    }
}

impl std::error::Error for ManagedToolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Http(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::DigestTool { source, .. } => Some(source),
            _ => None,
        }
    }
}
