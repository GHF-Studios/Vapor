//! Vapor-owned resolution and invocation for the Developer Git provider.
//!
//! Git is a Developer capability. Vapor selects an explicit environment
//! override, an operator-linked executable, or the managed executable under
//! User Data, in that order. It never adopts host `PATH` implicitly.

use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

const PROVIDER_FILE: &str = "providers/git.toml";
const GIT_ENV: &str = "VAPOR_GIT";
const MANAGED_GIT_DIRECTORY: &str = "tools/git";

#[derive(Debug, Clone)]
pub struct GitProvider {
    path: PathBuf,
    source: GitProviderSource,
    user_data_root: PathBuf,
}

impl GitProvider {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn source(&self) -> GitProviderSource {
        self.source
    }

    /// Run a Git process with the runtime environment declared by its provider.
    pub fn run<I, S>(&self, arguments: I) -> Result<ExitStatus, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut command = Command::new(&self.path);
        command.args(arguments);
        self.configure_command(&mut command)?;
        command.status().map_err(|error| {
            format!("failed to start Git provider '{}': {error}", self.path.display())
        })
    }

    /// Run a Git process when the caller needs to inspect its output.
    pub fn run_output<I, S>(&self, arguments: I) -> Result<std::process::Output, std::io::Error>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut command = Command::new(&self.path);
        command.args(arguments);
        self.configure_command(&mut command)
            .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
        command.output()
    }

    fn configure_command(&self, command: &mut Command) -> Result<(), String> {
        let root = self.user_data_root.join(MANAGED_GIT_DIRECTORY);
        if self.source == GitProviderSource::Managed {
            let paths = vec![self.path.parent().unwrap_or(&root).to_path_buf()];
            #[cfg(windows)]
            let paths = {
                let mut paths = paths;
                paths.extend([
                    root.join("usr/bin"),
                    root.join("ucrt64/bin"),
                    root.join("mingw64/bin"),
                    root.join("cmd"),
                ]);
                paths
            };
            append_existing_path(command, paths)?;

            let executable_root = executable_root(&root);

            if let Some(executable_root) = executable_root {
                let exec_path = executable_root.join("libexec/git-core");
                if exec_path.is_dir() {
                    command.env("GIT_EXEC_PATH", exec_path);
                }
                let template_path = executable_root.join("share/git-core/templates");
                if template_path.is_dir() {
                    command.env("GIT_TEMPLATE_DIR", template_path);
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitProviderSource {
    Environment,
    Linked,
    Managed,
}

impl GitProviderSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Environment => "VAPOR_GIT",
            Self::Linked => "linked provider",
            Self::Managed => "Vapor User Data",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GitProviderConfig {
    path: PathBuf,
}

/// Resolve Git without implicit host-PATH or common-location discovery.
pub fn resolve(user_data_root: &Path, state_root: &Path) -> Result<GitProvider, String> {
    if let Some(value) = env::var_os(GIT_ENV).filter(|value| !value.is_empty()) {
        return verify(PathBuf::from(value), GitProviderSource::Environment, user_data_root);
    }

    if let Some(path) = linked_path(state_root)? {
        return verify(path, GitProviderSource::Linked, user_data_root);
    }

    let root = user_data_root.join(MANAGED_GIT_DIRECTORY);
    let managed = if cfg!(windows) {
        root.join("cmd").join(executable("git"))
    } else {
        root.join("bin").join(executable("git"))
    };
    if managed.is_file() {
        return verify(managed, GitProviderSource::Managed, user_data_root);
    }

    Err(format!(
        "Vapor-managed Git is not installed and no explicit provider is linked\nhelp: Developer setup must provision Git under Vapor User Data; an operator may explicitly link an existing Git with `provider git link /path/to/git` or set {GIT_ENV}=/path/to/git\nnote: Vapor does not silently use host PATH Git, and normal game play does not require Git"
    ))
}

/// Persist an explicitly selected operator provider.
pub fn link(
    user_data_root: &Path,
    state_root: &Path,
    path: &Path,
) -> Result<GitProvider, String> {
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("failed to resolve Git executable '{}': {error}", path.display()))?;
    let provider = verify(canonical, GitProviderSource::Linked, user_data_root)?;
    let target = config_path(state_root);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!("failed to create Git provider state directory '{}': {error}", parent.display())
        })?;
    }
    let source = toml::to_string_pretty(&GitProviderConfig {
        path: provider.path.clone(),
    })
    .map_err(|error| format!("failed to encode Git provider state: {error}"))?;
    fs::write(&target, source)
        .map_err(|error| format!("failed to write Git provider state '{}': {error}", target.display()))?;
    Ok(provider)
}

/// Remove the persisted explicit provider link.
pub fn clear(state_root: &Path) -> Result<bool, String> {
    let target = config_path(state_root);
    if !target.exists() {
        return Ok(false);
    }
    fs::remove_file(&target)
        .map_err(|error| format!("failed to remove Git provider state '{}': {error}", target.display()))?;
    Ok(true)
}

/// Read an explicit provider link without validating its executable.
pub fn linked_path(state_root: &Path) -> Result<Option<PathBuf>, String> {
    let target = config_path(state_root);
    if !target.is_file() {
        return Ok(None);
    }
    let source = fs::read_to_string(&target)
        .map_err(|error| format!("failed to read Git provider state '{}': {error}", target.display()))?;
    let config = toml::from_str::<GitProviderConfig>(&source)
        .map_err(|error| format!("failed to parse Git provider state '{}': {error}", target.display()))?;
    Ok(Some(config.path))
}

fn config_path(state_root: &Path) -> PathBuf {
    state_root.join(PROVIDER_FILE)
}

fn verify(
    path: PathBuf,
    source: GitProviderSource,
    user_data_root: &Path,
) -> Result<GitProvider, String> {
    if !is_executable(&path) {
        return Err(format!("Git provider is not executable: {}", path.display()));
    }
    let provider = GitProvider {
        path,
        source,
        user_data_root: user_data_root.to_path_buf(),
    };
    let mut command = Command::new(provider.path());
    command.arg("--version");
    provider.configure_command(&mut command)?;
    let output = command.output().map_err(|error| {
        format!("failed to start Git '{}': {error}", provider.path.display())
    })?;
    if !output.status.success() {
        return Err(format!(
            "Git provider '{}' failed health check with {}",
            provider.path.display(),
            output.status
        ));
    }
    Ok(provider)
}

fn executable(name: &str) -> String {
    format!("{name}{}", env::consts::EXE_SUFFIX)
}

fn executable_root(root: &Path) -> Option<PathBuf> {
    // Git for Windows' stable launcher stays in cmd/, while current x64
    // MinGit stores helper binaries under ucrt64/. Retain mingw64/ for older
    // managed payloads that used the previous x64 environment.
    let candidates = if cfg!(windows) {
        vec![root.join("ucrt64"), root.join("mingw64"), root.to_path_buf()]
    } else {
        vec![root.to_path_buf()]
    };
    candidates
        .into_iter()
        .find(|directory| directory.join("libexec/git-core").is_dir())
}

fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(windows)]
fn append_existing_path(command: &mut Command, paths: Vec<PathBuf>) -> Result<(), String> {
    let existing = env::var_os("PATH").unwrap_or_default();
    let mut all = paths;
    all.extend(env::split_paths(&existing));
    let joined = env::join_paths(all).map_err(|error| format!("failed to prepare MinGit PATH: {error}"))?;
    command.env("PATH", joined);
    Ok(())
}

#[cfg(not(windows))]
fn append_existing_path(_command: &mut Command, _paths: Vec<PathBuf>) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{GitProvider, GitProviderSource};
    use std::path::PathBuf;

    #[cfg(unix)]
    #[test]
    fn provider_output_captures_stdout_and_exit_status() {
        let provider = GitProvider {
            path: PathBuf::from("/bin/sh"),
            source: GitProviderSource::Environment,
            user_data_root: PathBuf::new(),
        };

        let output = provider
            .run_output(["-c", "printf provider-output; exit 7"])
            .expect("start provider process");

        assert_eq!(output.stdout, b"provider-output");
        assert_eq!(output.status.code(), Some(7));
    }
}
