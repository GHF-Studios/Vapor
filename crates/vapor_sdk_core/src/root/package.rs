use std::fs;
use std::path::{Path, PathBuf};

use crate::toolchain::toolchain_status;

use super::error::RootCommandError;
use super::types::{RootPackageReport, RootPackageRequest};

const ROOT_OUTPUT_DIR: &str = "root";
const CONTENT_DIR: &str = "content";
const SCRIPTS_DIR: &str = "scripts";
const STEAM_BUILD_DIR: &str = "steam-build";
const BIN_DIR: &str = "bin";
const APP_BINARIES: [&str; 6] = [
    "vapor",
    "vapor-gui",
    "vapor-installer",
    "vapor-entrypoint",
    "vapor-ide-host",
    "vapor-devtools",
];
const ACTIVATION_SCRIPT_UNIX: &str = "vapor_env.sh";
const ACTIVATION_SCRIPT_WINDOWS: &str = "vapor_env.cmd";

/// Assemble the Steam redistributable root package.
pub fn root_package(request: &RootPackageRequest) -> Result<RootPackageReport, RootCommandError> {
    let layout = RootPackageLayout::new(request)?;
    layout.package(request)
}

fn validate_ids(app_id: u32, depot_id: u32) -> Result<(), RootCommandError> {
    if app_id == 0 {
        return Err(RootCommandError::InvalidAppId(app_id));
    }

    if depot_id == 0 {
        return Err(RootCommandError::InvalidDepotId(depot_id));
    }

    Ok(())
}

fn validate_set_live(set_live: Option<&str>) -> Result<(), RootCommandError> {
    if set_live.is_some_and(|branch| branch == "default") {
        return Err(RootCommandError::DefaultBranchCannotBeSetLive);
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct RootPackageLayout {
    vapor_home: PathBuf,
    host_triple: String,
    package_root: PathBuf,
    content_root: PathBuf,
    scripts_root: PathBuf,
    steam_build_root: PathBuf,
    app_build_script: PathBuf,
    included_roots: Vec<PackageRoot>,
}

impl RootPackageLayout {
    fn new(request: &RootPackageRequest) -> Result<Self, RootCommandError> {
        validate_ids(request.app_id, request.depot_id)?;
        validate_set_live(request.set_live.as_deref())?;

        let status = toolchain_status()?;
        let package_root = status.output_root.join(ROOT_OUTPUT_DIR);
        let content_root = package_root.join(CONTENT_DIR);
        let scripts_root = package_root.join(SCRIPTS_DIR);
        let steam_build_root = package_root.join(STEAM_BUILD_DIR);
        let app_build_script = scripts_root.join(format!("app_build_{}.vdf", request.app_id));
        let included_roots = package_roots(&status.vapor_home);

        for root in &included_roots {
            if !root.source.exists() {
                return Err(RootCommandError::MissingPackageInput(root.source.clone()));
            }
        }

        Ok(Self {
            vapor_home: status.vapor_home,
            host_triple: status.host_triple.to_owned(),
            package_root,
            content_root,
            scripts_root,
            steam_build_root,
            app_build_script,
            included_roots,
        })
    }

    fn package(&self, request: &RootPackageRequest) -> Result<RootPackageReport, RootCommandError> {
        let mut copied_files = 0;

        if !request.plan {
            reset_dir(&self.content_root)?;
            reset_dir(&self.scripts_root)?;
            fs::create_dir_all(&self.steam_build_root)?;
        }

        for root in &self.included_roots {
            let target = self.content_root.join(&root.depot_path);
            copied_files += copy_tree(&root.source, &target, request.plan)?;
        }

        if !request.plan {
            write_app_build_script(
                &self.app_build_script,
                request,
                &self.content_root,
                &self.steam_build_root,
            )?;
        }

        Ok(RootPackageReport {
            planned: request.plan,
            app_id: request.app_id,
            depot_id: request.depot_id,
            host_triple: self.host_triple.clone(),
            description: request.description.clone(),
            set_live: request.set_live.clone(),
            vapor_home: self.vapor_home.clone(),
            package_root: self.package_root.clone(),
            content_root: self.content_root.clone(),
            scripts_root: self.scripts_root.clone(),
            steam_build_root: self.steam_build_root.clone(),
            app_build_script: self.app_build_script.clone(),
            included_roots: self
                .included_roots
                .iter()
                .map(|root| root.depot_path.clone())
                .collect(),
            copied_files,
        })
    }
}

fn package_roots(vapor_home: &Path) -> Vec<PackageRoot> {
    let mut roots = APP_BINARIES
        .iter()
        .map(|binary| {
            let relative = PathBuf::from(BIN_DIR).join(executable_name(binary));
            PackageRoot::new(vapor_home.join(&relative), relative)
        })
        .collect::<Vec<_>>();
    roots.push(PackageRoot::new(
        vapor_home.join(activation_script_name()),
        PathBuf::from(activation_script_name()),
    ));
    roots
}

#[derive(Debug, Clone)]
struct PackageRoot {
    source: PathBuf,
    depot_path: PathBuf,
}

impl PackageRoot {
    fn new(source: PathBuf, depot_path: PathBuf) -> Self {
        Self { source, depot_path }
    }
}

fn reset_dir(path: &Path) -> Result<(), RootCommandError> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }

    fs::create_dir_all(path)?;
    Ok(())
}

fn copy_tree(source: &Path, target: &Path, plan: bool) -> Result<usize, RootCommandError> {
    let metadata = fs::symlink_metadata(source)?;

    if metadata.is_dir() {
        if !plan {
            fs::create_dir_all(target)?;
        }

        let mut copied_files = 0;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copied_files += copy_tree(&entry.path(), &target.join(entry.file_name()), plan)?;
        }
        return Ok(copied_files);
    }

    if metadata.file_type().is_symlink() {
        return copy_symlink_target(source, target, plan);
    }

    if metadata.is_file() {
        if !plan {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(source, target)?;
        }
        return Ok(1);
    }

    Ok(0)
}

fn copy_symlink_target(
    source: &Path,
    target: &Path,
    plan: bool,
) -> Result<usize, RootCommandError> {
    let link_target = fs::read_link(source)?;
    let resolved_target = if link_target.is_absolute() {
        link_target
    } else {
        source
            .parent()
            .ok_or_else(|| RootCommandError::SymlinkHasNoParent(source.to_path_buf()))?
            .join(link_target)
    };

    copy_tree(&resolved_target, target, plan)
}

fn write_app_build_script(
    path: &Path,
    request: &RootPackageRequest,
    content_root: &Path,
    steam_build_root: &Path,
) -> Result<(), RootCommandError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let set_live = request.set_live.as_deref().map_or(String::new(), |branch| {
        format!("    \"SetLive\" \"{}\"\n", vdf_escape(branch))
    });
    let script = format!(
        "\"AppBuild\"\n{{\n    \"AppID\" \"{}\"\n    \"Desc\" \"{}\"\n{}    \"ContentRoot\" \"{}\"\n    \"BuildOutput\" \"{}\"\n    \"Depots\"\n    {{\n        \"{}\"\n        {{\n            \"FileMapping\"\n            {{\n                \"LocalPath\" \"*\"\n                \"DepotPath\" \".\"\n                \"recursive\" \"1\"\n            }}\n        }}\n    }}\n}}\n",
        request.app_id,
        vdf_escape(&request.description),
        set_live,
        vdf_escape(&content_root.display().to_string()),
        vdf_escape(&steam_build_root.display().to_string()),
        request.depot_id,
    );

    fs::write(path, script)?;
    Ok(())
}

fn vdf_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn alias_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.cmd")
    } else {
        stem.to_owned()
    }
}

fn executable_name(stem: &str) -> String {
    format!("{stem}{}", std::env::consts::EXE_SUFFIX)
}

fn activation_script_name() -> &'static str {
    if cfg!(windows) {
        ACTIVATION_SCRIPT_WINDOWS
    } else {
        ACTIVATION_SCRIPT_UNIX
    }
}
