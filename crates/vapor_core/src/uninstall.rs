//! Vapor installation teardown and explicit purge operations.
//!
//! Ordinary uninstall removes machine integration only. Vapor-owned OS user
//! data and authored source survive unless explicitly purged.

use crate::installation::{InstallationError, VAPOR_HOME_ENV, VaporInstallation};
use crate::source::{SourceError, source_state};
use crate::superworkspace::{
    SUPERWORKSPACE_MANIFEST_FILE_NAME, SuperworkspaceError, VaporSuperworkspace,
};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const LEGACY_APP_ROOT_ENV: &str = "LOO_CAST_APP_ROOT";
const PROFILE_BLOCK_START: &str = "# >>> Vapor managed PATH >>>";
const PROFILE_BLOCK_END: &str = "# <<< Vapor managed PATH <<<";
const LEGACY_APP_MUTABLE_DIRS: &[&str] = &["rustup-home", "cargo-home", "development"];
const MANAGED_SUPERWORKSPACE_REPOSITORIES: &[&str] =
    &["Loo-Cast", "Vapor-Client", "Vapor-Platform-Server"];

#[derive(Debug, Clone, Copy, Default)]
pub struct UninstallOptions {
    pub purge_app_external: bool,
    pub purge_superworkspace: bool,
}

#[derive(Debug, Clone)]
pub struct UninstallReport {
    pub installation_root: PathBuf,
    pub user_data_root: PathBuf,
    pub superworkspace_root: Option<PathBuf>,
    pub integration: Vec<String>,
    pub legacy_app_state_removed: Vec<PathBuf>,
    pub app_external_purged: bool,
    pub superworkspace_purged: bool,
}

pub fn uninstall_installation(
    options: UninstallOptions,
) -> Result<UninstallReport, UninstallError> {
    let installation = VaporInstallation::discover().map_err(UninstallError::Installation)?;
    let user_data_root = installation.user_data_root();

    let superworkspace_root = if options.purge_superworkspace {
        Some(active_superworkspace(&installation)?)
    } else {
        None
    };

    // Phase 1: resolve and validate every safety-sensitive operation before
    // mutating integration, user data, or authored source.
    let preflight = preflight_uninstall(
        &installation,
        &user_data_root,
        superworkspace_root.as_deref(),
        options,
    )?;

    if !options.purge_app_external && installation.root.join("state").is_dir() {
        installation
            .ensure_state_root()
            .map_err(UninstallError::Installation)?;
    }

    let legacy_app_state_removed = remove_legacy_app_mutable_state(&installation)?;

    let mut app_external_purged = false;
    let mut superworkspace_purged = false;

    if let Some(superworkspace) = &preflight.superworkspace_purge_root {
        superworkspace_purged = remove_preflighted_tree(superworkspace)?;
    }

    if let Some(app_external) = &preflight.app_external_purge_root {
        app_external_purged = remove_preflighted_tree(app_external)?;

        let legacy_state = installation.root.join("state");
        if legacy_state.is_dir() {
            fs::remove_dir_all(&legacy_state).map_err(|source| UninstallError::Io {
                path: legacy_state,
                source,
            })?;
            app_external_purged = true;
        }
    }

    // Machine integration is committed last. Safety validation above has
    // already completed, so a refused purge cannot partially uninstall Vapor.
    let integration = remove_machine_integration(&installation)?;

    Ok(UninstallReport {
        installation_root: installation.root,
        user_data_root,
        superworkspace_root,
        integration,
        legacy_app_state_removed,
        app_external_purged,
        superworkspace_purged,
    })
}

#[derive(Debug)]
struct UninstallPreflight {
    superworkspace_purge_root: Option<PathBuf>,
    app_external_purge_root: Option<PathBuf>,
}

fn preflight_uninstall(
    installation: &VaporInstallation,
    user_data_root: &Path,
    superworkspace_root: Option<&Path>,
    options: UninstallOptions,
) -> Result<UninstallPreflight, UninstallError> {
    preflight_machine_integration_removal()?;

    let superworkspace_purge_root = if let Some(superworkspace) = superworkspace_root {
        let root = preflight_owned_tree(
            superworkspace,
            &[installation.root.as_path(), user_data_root],
        )?
        .ok_or_else(|| UninstallError::UnsafePurgeRoot {
            path: superworkspace.to_path_buf(),
        })?;

        ensure_superworkspace_purge_safe(&root)?;
        Some(root)
    } else {
        None
    };

    let app_external_purge_root = if options.purge_app_external {
        preflight_owned_tree(user_data_root, &[installation.root.as_path()])?
    } else {
        None
    };

    Ok(UninstallPreflight {
        superworkspace_purge_root,
        app_external_purge_root,
    })
}

fn preflight_owned_tree(
    path: &Path,
    protected: &[&Path],
) -> Result<Option<PathBuf>, UninstallError> {
    if !path.exists() {
        return Ok(None);
    }

    let path = fs::canonicalize(path).map_err(|source| UninstallError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    if path.parent().is_none() || home_dir().is_some_and(|home| same_path(&path, &home)) {
        return Err(UninstallError::UnsafePurgeRoot { path });
    }

    for protected in protected {
        let protected = fs::canonicalize(protected).unwrap_or_else(|_| (*protected).to_path_buf());
        if path.starts_with(&protected) || protected.starts_with(&path) {
            return Err(UninstallError::UnsafePurgeRoot { path });
        }
    }

    Ok(Some(path))
}

fn preflight_machine_integration_removal() -> Result<(), UninstallError> {
    if cfg!(windows) {
        return Ok(());
    }

    for profile in shell_profiles() {
        validate_managed_profile_block(&profile)?;
    }

    Ok(())
}

fn validate_managed_profile_block(path: &Path) -> Result<(), UninstallError> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(UninstallError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let mut inside = false;
    for line in source.lines() {
        if line == PROFILE_BLOCK_START {
            inside = true;
            continue;
        }

        if inside && line == PROFILE_BLOCK_END {
            inside = false;
        }
    }

    if inside {
        return Err(UninstallError::MalformedProfileBlock {
            path: path.to_path_buf(),
        });
    }

    Ok(())
}

fn active_superworkspace(installation: &VaporInstallation) -> Result<PathBuf, UninstallError> {
    let state = source_state(installation).map_err(UninstallError::Source)?;

    if let Some(configured) = state.superworkspace.as_deref()
        && let Ok(superworkspace) = VaporSuperworkspace::discover_from(configured)
    {
        return Ok(superworkspace.root);
    }

    if let Some(active) = state.active.as_deref()
        && let Ok(superworkspace) = VaporSuperworkspace::discover_from(active)
    {
        return Ok(superworkspace.root);
    }

    if let Ok(current) = env::current_dir()
        && let Ok(superworkspace) = VaporSuperworkspace::discover_from(&current)
    {
        return Ok(superworkspace.root);
    }

    let mut known = BTreeSet::new();
    for source in &state.known {
        if let Ok(superworkspace) = VaporSuperworkspace::discover_from(source) {
            known.insert(superworkspace.root);
        }
    }

    if known.len() == 1 {
        return Ok(known.into_iter().next().expect("length checked"));
    }

    Err(UninstallError::NoActiveSuperworkspace {
        state_path: state.state_path,
    })
}

fn remove_legacy_app_mutable_state(
    installation: &VaporInstallation,
) -> Result<Vec<PathBuf>, UninstallError> {
    let mut removed = Vec::new();

    for name in LEGACY_APP_MUTABLE_DIRS {
        let path = installation.root.join(name);

        if !path.exists() {
            continue;
        }

        fs::remove_dir_all(&path).map_err(|source| UninstallError::Io {
            path: path.clone(),
            source,
        })?;

        removed.push(path);
    }

    Ok(removed)
}

fn remove_machine_integration(
    installation: &VaporInstallation,
) -> Result<Vec<String>, UninstallError> {
    let mut removed = Vec::new();

    if let Some(anchor) = legacy_anchor_file() {
        if anchor.is_file() {
            fs::remove_file(&anchor).map_err(|source| UninstallError::Io {
                path: anchor.clone(),
                source,
            })?;
            removed.push(format!(
                "removed legacy app-root anchor {}",
                anchor.display()
            ));

            if let Some(parent) = anchor.parent() {
                let _ = fs::remove_dir(parent);
            }
        }
    }

    if cfg!(windows) {
        if remove_windows_user_path(installation)? {
            removed.push("removed Vapor host binary directory from user PATH".to_owned());
        }

        for variable in [VAPOR_HOME_ENV, LEGACY_APP_ROOT_ENV] {
            if remove_windows_user_environment(variable)? {
                removed.push(format!("removed user environment variable {variable}"));
            }
        }
    } else {
        for profile in shell_profiles() {
            if remove_managed_profile_block(&profile)? {
                removed.push(format!(
                    "removed managed Vapor PATH block from {}",
                    profile.display()
                ));
            }
        }
    }

    Ok(removed)
}

fn legacy_anchor_file() -> Option<PathBuf> {
    if cfg!(windows) {
        env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("USERPROFILE")
                    .map(|home| PathBuf::from(home).join("AppData/Roaming"))
            })
            .map(|root| root.join("loo_cast/app_root.path"))
    } else {
        env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|root| root.join("loo_cast/app_root.path"))
    }
}

fn shell_profiles() -> Vec<PathBuf> {
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };

    [".profile", ".bashrc", ".zshrc"]
        .into_iter()
        .map(|name| home.join(name))
        .collect()
}

fn remove_managed_profile_block(path: &Path) -> Result<bool, UninstallError> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(UninstallError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let mut changed = false;
    let mut inside = false;
    let mut output = String::new();

    for line in source.lines() {
        if line == PROFILE_BLOCK_START {
            changed = true;
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
        return Err(UninstallError::MalformedProfileBlock {
            path: path.to_path_buf(),
        });
    }

    if changed {
        fs::write(path, output).map_err(|source| UninstallError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }

    Ok(changed)
}

fn remove_windows_user_path(installation: &VaporInstallation) -> Result<bool, UninstallError> {
    let target = if cfg!(all(
        target_arch = "x86_64",
        target_os = "windows",
        target_env = "msvc"
    )) {
        "x86_64-pc-windows-msvc"
    } else {
        return Ok(false);
    };

    let bin = installation.root.join("bin").join(target);
    let legacy = format!(r"%VAPOR_HOME%\bin\{target}");
    let script = r#"
$bin = $args[0]
$legacy = $args[1]
$path = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($null -eq $path) { exit 0 }
$entries = @($path -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
$filtered = @($entries | Where-Object {
    -not [string]::Equals($_, $bin, [StringComparison]::OrdinalIgnoreCase) -and
    -not [string]::Equals($_, $legacy, [StringComparison]::OrdinalIgnoreCase)
})
if ($filtered.Count -ne $entries.Count) {
    [Environment]::SetEnvironmentVariable('Path', ($filtered -join ';'), 'User')
    Write-Output 'removed'
}
"#;

    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .arg(&bin)
        .arg(&legacy)
        .output()
        .map_err(|source| UninstallError::CommandStart {
            command: "PowerShell user PATH cleanup".to_owned(),
            source,
        })?;

    if !output.status.success() {
        return Err(UninstallError::CommandFailed {
            command: "PowerShell user PATH cleanup".to_owned(),
            status: output.status.to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).contains("removed"))
}

fn remove_windows_user_environment(variable: &str) -> Result<bool, UninstallError> {
    let key = r"HKCU\Environment";
    let query = Command::new("reg")
        .args(["query", key, "/V", variable])
        .output()
        .map_err(|source| UninstallError::CommandStart {
            command: format!("reg query {key} /V {variable}"),
            source,
        })?;

    if !query.status.success() {
        return Ok(false);
    }

    let delete = Command::new("reg")
        .args(["delete", key, "/V", variable, "/F"])
        .output()
        .map_err(|source| UninstallError::CommandStart {
            command: format!("reg delete {key} /V {variable} /F"),
            source,
        })?;

    if !delete.status.success() {
        return Err(UninstallError::CommandFailed {
            command: format!("reg delete {key} /V {variable} /F"),
            status: delete.status.to_string(),
        });
    }

    Ok(true)
}

fn ensure_superworkspace_purge_safe(root: &Path) -> Result<(), UninstallError> {
    let root = fs::canonicalize(root).map_err(|source| UninstallError::Io {
        path: root.to_path_buf(),
        source,
    })?;

    for entry in fs::read_dir(&root).map_err(|source| UninstallError::Io {
        path: root.clone(),
        source,
    })? {
        let entry = entry.map_err(|source| UninstallError::Io {
            path: root.clone(),
            source,
        })?;
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(UninstallError::UnsafeSuperworkspaceEntry { path });
        };

        if matches!(name, SUPERWORKSPACE_MANIFEST_FILE_NAME | ".vaporignore") {
            continue;
        }

        let file_type = entry.file_type().map_err(|source| UninstallError::Io {
            path: path.clone(),
            source,
        })?;

        if matches!(name, ".idea" | ".run") {
            if !file_type.is_dir() || file_type.is_symlink() {
                return Err(UninstallError::UnsafeSuperworkspaceEntry { path });
            }
            continue;
        }

        if !MANAGED_SUPERWORKSPACE_REPOSITORIES.contains(&name)
            || !file_type.is_dir()
            || file_type.is_symlink()
        {
            return Err(UninstallError::UnsafeSuperworkspaceEntry { path });
        }

        ensure_expected_origin(name, &path)?;
        ensure_source_checkout_safe(name, &path)?;
    }

    Ok(())
}

fn ensure_expected_origin(name: &str, root: &Path) -> Result<(), UninstallError> {
    let origin = git_output(root, &["remote", "get-url", "origin"])?;
    let normalized = origin
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let expected = format!("GHF-Studios/{name}");

    if !normalized.ends_with(&expected) {
        return Err(UninstallError::UnsafeSource {
            message: format!(
                "refusing to purge `{}` because origin `{}` does not match expected first-party repository `{expected}`",
                root.display(),
                origin.trim(),
            ),
        });
    }

    Ok(())
}

fn ensure_source_checkout_safe(name: &str, root: &Path) -> Result<(), UninstallError> {
    ensure_git_checkout_safe(name, root)?;

    let submodules = git_output(root, &["submodule", "status"])?;
    for line in submodules.lines() {
        let Some(prefix) = line.chars().next() else {
            continue;
        };

        if prefix == '-' {
            continue;
        }

        if matches!(prefix, '+' | 'U') {
            return Err(UninstallError::UnsafeSource {
                message: format!(
                    "refusing to purge source `{name}` because direct submodule state is not clean: {line}"
                ),
            });
        }

        let mut fields = line[1..].split_whitespace();
        let _commit = fields.next();
        let Some(path) = fields.next() else {
            continue;
        };
        let submodule = root.join(path);
        if submodule.is_dir() {
            ensure_git_checkout_safe(&format!("{name}/{path}"), &submodule)?;
        }
    }

    Ok(())
}

fn ensure_git_checkout_safe(name: &str, root: &Path) -> Result<(), UninstallError> {
    let inside = git_output(root, &["rev-parse", "--is-inside-work-tree"])?;
    if inside != "true" {
        return Err(UninstallError::UnsafeSource {
            message: format!("source `{name}` is not a Git work tree: `{}`", root.display()),
        });
    }

    let dirty = git_output(
        root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    if !dirty.is_empty() {
        return Err(UninstallError::UnsafeSource {
            message: format!(
                "refusing to purge source `{name}` because it has uncommitted/untracked changes:\n{dirty}"
            ),
        });
    }

    git_output(root, &["fetch", "--quiet", "--prune", "origin"])?;

    let local_only = git_output(root, &["rev-list", "--branches", "--not", "--remotes=origin"])?;
    if !local_only.is_empty() {
        return Err(UninstallError::UnsafeSource {
            message: format!(
                "refusing to purge source `{name}` because local branches contain commits not present on origin"
            ),
        });
    }

    let remote_contains_head = git_output(root, &["branch", "-r", "--contains", "HEAD"])?;
    if remote_contains_head.is_empty() {
        return Err(UninstallError::UnsafeSource {
            message: format!(
                "refusing to purge source `{name}` because HEAD is not reachable from an origin branch"
            ),
        });
    }

    let local_tags = git_output(
        root,
        &[
            "for-each-ref",
            "--format=%(refname:strip=2) %(objectname)",
            "refs/tags",
        ],
    )?;
    let remote_tags = git_output(root, &["ls-remote", "--tags", "--refs", "origin"])?;

    let remote_tags = remote_tags
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let object = fields.next()?;
            let reference = fields.next()?;
            Some((
                reference.trim_start_matches("refs/tags/").to_owned(),
                object.to_owned(),
            ))
        })
        .collect::<BTreeMap<_, _>>();

    for line in local_tags.lines() {
        let Some((tag, object)) = line.split_once(' ') else {
            continue;
        };
        if remote_tags.get(tag).map(String::as_str) != Some(object) {
            return Err(UninstallError::UnsafeSource {
                message: format!(
                    "refusing to purge source `{name}` because local tag `{tag}` is not identically published on origin"
                ),
            });
        }
    }

    Ok(())
}

fn git_output(root: &Path, args: &[&str]) -> Result<String, UninstallError> {
    let command = format!("git -C {} {}", root.display(), args.join(" "));
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|source| UninstallError::CommandStart {
            command: command.clone(),
            source,
        })?;

    if !output.status.success() {
        return Err(UninstallError::CommandFailed {
            command,
            status: output.status.to_string(),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn remove_preflighted_tree(path: &Path) -> Result<bool, UninstallError> {
    if !path.exists() {
        return Ok(false);
    }

    fs::remove_dir_all(path).map_err(|source| UninstallError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    Ok(true)
}

fn same_path(left: &Path, right: &Path) -> bool {
    fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf())
        == fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf())
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[derive(Debug)]
pub enum UninstallError {
    Installation(InstallationError),
    Source(SourceError),
    Superworkspace(SuperworkspaceError),
    NoActiveSuperworkspace { state_path: PathBuf },
    UnsafePurgeRoot { path: PathBuf },
    UnsafeSuperworkspaceEntry { path: PathBuf },
    UnsafeSource { message: String },
    MalformedProfileBlock { path: PathBuf },
    CommandStart { command: String, source: io::Error },
    CommandFailed { command: String, status: String },
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for UninstallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Installation(error) => error.fmt(formatter),
            Self::Source(error) => error.fmt(formatter),
            Self::Superworkspace(error) => error.fmt(formatter),
            Self::NoActiveSuperworkspace { state_path } => write!(
                formatter,
                "--purge-superworkspace could not resolve the canonical Superworkspace (source state: `{}`)",
                state_path.display(),
            ),
            Self::UnsafePurgeRoot { path } => write!(
                formatter,
                "refusing unsafe purge root `{}`",
                path.display()
            ),
            Self::UnsafeSuperworkspaceEntry { path } => write!(
                formatter,
                "refusing to purge Superworkspace because it contains unrecognized entry `{}`",
                path.display()
            ),
            Self::UnsafeSource { message } => formatter.write_str(message),
            Self::MalformedProfileBlock { path } => write!(
                formatter,
                "managed Vapor PATH block in `{}` has no closing marker; refusing to rewrite it",
                path.display()
            ),
            Self::CommandStart { command, source } => {
                write!(formatter, "failed to start `{command}`: {source}")
            }
            Self::CommandFailed { command, status } => {
                write!(formatter, "`{command}` failed with {status}")
            }
            Self::Io { path, source } => {
                write!(formatter, "failed to modify `{}`: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for UninstallError {
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
