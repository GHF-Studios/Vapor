//! SteamCMD-backed beta-branch SteamPipe publishing.

use crate::{
    discovery::{EnvironmentPaths, InstallationPaths, ensure_contained},
    distribution::{self, DistributionManifest, StageOptions, SteamDepotKind},
    manifest,
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const STEAMPIPE_TEMPLATE_ROOT: &str = "resources/steam/steampipe-templates";
const APP_BUILD_TEMPLATE: &str = "app_build.vdf.tpl";
const APP_BUILD_DEPOT_ENTRY_TEMPLATE: &str = "app_build_depot_entry.vdf.tpl";
const DEPOT_BUILD_TEMPLATE: &str = "depot_build.vdf.tpl";

/// Locate the installation-owned SteamCMD executable.
pub fn executable(paths: &EnvironmentPaths) -> Result<PathBuf, String> {
    executable_for_installation(paths.installation())
}

/// Locate the installation-owned SteamCMD executable from an app root.
pub fn executable_for_installation(installation: &InstallationPaths) -> Result<PathBuf, String> {
    let root = installation.root();
    let names = if cfg!(windows) {
        vec!["steamcmd.exe"]
    } else {
        vec!["steamcmd", "steamcmd.sh"]
    };
    [root.join("tools/steamcmd"), root.join("steam/steamcmd")]
        .into_iter()
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|path| path.is_file())
        .ok_or_else(|| "SteamCMD is not installed in the Vapor app root".to_owned())
}

/// Options for a SteamPipe publication attempt.
#[derive(Debug, Clone)]
pub struct PublishOptions<'a> {
    /// Dedicated Steam build account.
    pub account: &'a str,
    /// Target beta branch, or the manifest default.
    pub branch: Option<&'a str>,
    /// Internal Steam build description.
    pub description: &'a str,
    /// Staged payload mode.
    pub stage_options: StageOptions,
    /// Whether to generate a preview VDF without uploading.
    pub dry_run: bool,
    /// Whether a real upload has been explicitly confirmed.
    pub confirmed: bool,
}

/// Result of a SteamPipe publish or dry-run preview.
#[derive(Debug, Clone)]
pub struct PublishReport {
    script: PathBuf,
    stage: distribution::StageReport,
}

impl PublishReport {
    /// Generated SteamPipe app build script.
    pub fn script(&self) -> &Path {
        &self.script
    }

    /// Staged depot content roots used by this publish attempt.
    pub fn stage(&self) -> &distribution::StageReport {
        &self.stage
    }
}

/// Stage, validate, and publish the development beta through SteamCMD.
pub fn publish(
    paths: &EnvironmentPaths,
    manifest: &DistributionManifest,
    options: PublishOptions<'_>,
) -> Result<PublishReport, String> {
    let branch = options
        .branch
        .unwrap_or(manifest.application().development_branch());
    if branch == "default" || branch.trim().is_empty() {
        return Err("automatic publishing requires a non-default beta branch".to_owned());
    }
    if !options.dry_run && !options.confirmed {
        return Err("publishing requires --yes after reviewing --dry-run".to_owned());
    }
    let stage = distribution::stage_with_options(paths, manifest, options.stage_options.clone())?;
    smoke(&stage, &options.stage_options)?;
    let script = write_build_script(
        paths,
        manifest,
        &stage,
        branch,
        options.description,
        options.dry_run,
    )?;
    if options.dry_run {
        return Ok(PublishReport { script, stage });
    }
    let steamcmd = executable(paths)?;
    let status = Command::new(&steamcmd)
        .args(["+login", options.account, "+run_app_build"])
        .arg(&script)
        .arg("+quit")
        .current_dir(steamcmd.parent().expect("SteamCMD has a parent"))
        .status()
        .map_err(|error| format!("failed to start SteamCMD: {error}"))?;
    if status.success() {
        Ok(PublishReport { script, stage })
    } else {
        Err(format!("Steam publish exited with {status}"))
    }
}

/// Validate essential staged self-hosting inputs.
pub fn smoke(stage: &distribution::StageReport, options: &StageOptions) -> Result<(), String> {
    let common = stage
        .depot(SteamDepotKind::Common)
        .ok_or_else(|| "staged application has no common depot".to_owned())?;
    for required in [manifest::APP_FILE_NAME, "docs"] {
        let path = common.root().join(required);
        if !path.exists() {
            return Err(format!(
                "staged common depot is missing required path: {}",
                path.display()
            ));
        }
    }

    for target in options.runtime_targets() {
        let kind = SteamDepotKind::for_runtime_target(target)?;
        let depot = stage
            .depot(kind)
            .ok_or_else(|| format!("staged application has no {} depot", kind.label()))?;
        match kind {
            SteamDepotKind::Common => {}
            SteamDepotKind::Linux => {
                let launch_script = depot.root().join("bin/vapor-launch.sh");
                require_file(
                    launch_script.clone(),
                    "staged Linux depot has no launch script",
                )?;
                require_script_target(
                    &launch_script,
                    &format!("target=\"{target}\""),
                    "staged Linux launch script selects the wrong runtime target",
                )?;
                require_file(
                    depot
                        .root()
                        .join("bin")
                        .join(target)
                        .join("vapor-entrypoint"),
                    "staged Linux depot has no target Vapor entrypoint",
                )?;
                require_file(
                    depot.root().join("bin").join(target).join("vapor"),
                    "staged Linux depot has no target Vapor binary",
                )?;
                require_file(
                    depot
                        .root()
                        .join("bin")
                        .join(target)
                        .join("vapor-installer"),
                    "staged Linux depot has no target Vapor Installer binary",
                )?;
                require_file(
                    depot.root().join("bin").join(target).join("vapor-gui"),
                    "staged Linux depot has no target Vapor GUI binary",
                )?;
                require_file(
                    depot.root().join("bin").join(target).join("vapor-ide-host"),
                    "staged Linux depot has no target Vapor IDE host binary",
                )?;
                require_file(
                    depot
                        .root()
                        .join("bin")
                        .join(target)
                        .join("vapor-client-service"),
                    "staged Linux depot has no target Vapor Studio service binary",
                )?;
            }
            SteamDepotKind::Windows => {
                let launch_script = depot.root().join("bin/vapor-launch.cmd");
                require_file(
                    launch_script.clone(),
                    "staged Windows depot has no launch script",
                )?;
                require_script_target(
                    &launch_script,
                    &format!("set \"TARGET={target}\""),
                    "staged Windows launch script selects the wrong runtime target",
                )?;
                require_file(
                    depot
                        .root()
                        .join("bin")
                        .join(target)
                        .join("vapor-entrypoint.exe"),
                    "staged Windows depot has no target Vapor entrypoint",
                )?;
                require_file(
                    depot.root().join("bin").join(target).join("vapor.exe"),
                    "staged Windows depot has no target Vapor binary",
                )?;
                require_file(
                    depot
                        .root()
                        .join("bin")
                        .join(target)
                        .join("vapor-installer.exe"),
                    "staged Windows depot has no target Vapor Installer binary",
                )?;
                require_file(
                    depot.root().join("bin").join(target).join("vapor-gui.exe"),
                    "staged Windows depot has no target Vapor GUI binary",
                )?;
                require_file(
                    depot
                        .root()
                        .join("bin")
                        .join(target)
                        .join("vapor-ide-host.exe"),
                    "staged Windows depot has no target Vapor IDE host binary",
                )?;
                require_file(
                    depot
                        .root()
                        .join("bin")
                        .join(target)
                        .join("vapor-client-service.exe"),
                    "staged Windows depot has no target Vapor Studio service binary",
                )?;
                require_file(
                    depot.root().join("bin").join(target).join("libunwind.dll"),
                    "staged Windows depot has no GNU runtime DLL",
                )?;
            }
        }
    }
    Ok(())
}

fn require_file(path: PathBuf, message: &str) -> Result<(), String> {
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("{message}: {}", path.display()))
    }
}

fn require_script_target(path: &Path, expected_line: &str, message: &str) -> Result<(), String> {
    let script = fs::read_to_string(path).map_err(|error| {
        format!(
            "failed to read staged launch script '{}': {error}",
            path.display()
        )
    })?;
    if script.lines().any(|line| line.trim() == expected_line) {
        Ok(())
    } else {
        Err(format!("{message}: {}", path.display()))
    }
}

fn write_build_script(
    paths: &EnvironmentPaths,
    manifest: &DistributionManifest,
    stage: &distribution::StageReport,
    branch: &str,
    description: &str,
    preview: bool,
) -> Result<PathBuf, String> {
    let templates = SteamPipeTemplates::load(paths.source().root())?;
    let scripts = paths.installation().root().join("output/root/scripts");
    let output = paths.installation().root().join("output/root/steam-build");
    fs::create_dir_all(&scripts).map_err(|e| e.to_string())?;
    fs::create_dir_all(&output).map_err(|e| e.to_string())?;
    let path = scripts.join(format!("app_build_{}.vdf", manifest.application().app_id()));
    let preview = if preview {
        "    \"Preview\" \"1\"\n"
    } else {
        ""
    };
    let mut depot_entries = String::new();
    for depot in stage.depots() {
        let depot_id = manifest.application().depot_id(depot.kind());
        let depot_script = scripts.join(format!("depot_build_{depot_id}.vdf"));
        let depot_vdf = render_template(
            &templates.depot_build,
            &[
                ("depot_id", depot_id.to_string()),
                ("content_root", escape(&depot.root().display().to_string())),
            ],
        );
        fs::write(&depot_script, depot_vdf).map_err(|e| e.to_string())?;
        depot_entries.push_str(&render_template(
            &templates.app_build_depot_entry,
            &[
                ("depot_id", depot_id.to_string()),
                (
                    "depot_build_script",
                    escape(&depot_script.display().to_string()),
                ),
            ],
        ));
    }
    let vdf = render_template(
        &templates.app_build,
        &[
            ("app_id", manifest.application().app_id().to_string()),
            ("description", escape(description)),
            ("preview_line", preview.to_owned()),
            ("branch", escape(branch)),
            ("content_root", escape(&stage.root().display().to_string())),
            ("build_output", escape(&output.display().to_string())),
            ("depot_entries", depot_entries),
        ],
    );
    fs::write(&path, vdf).map_err(|e| e.to_string())?;
    Ok(path)
}

struct SteamPipeTemplates {
    app_build: String,
    app_build_depot_entry: String,
    depot_build: String,
}

impl SteamPipeTemplates {
    fn load(source_root: &Path) -> Result<Self, String> {
        let root = source_root.join(STEAMPIPE_TEMPLATE_ROOT);
        ensure_contained(source_root, &root)?;
        Ok(Self {
            app_build: read_template(&root, APP_BUILD_TEMPLATE)?,
            app_build_depot_entry: read_template(&root, APP_BUILD_DEPOT_ENTRY_TEMPLATE)?,
            depot_build: read_template(&root, DEPOT_BUILD_TEMPLATE)?,
        })
    }
}

fn read_template(root: &Path, name: &str) -> Result<String, String> {
    let path = root.join(name);
    fs::read_to_string(&path).map_err(|error| {
        format!(
            "failed to read SteamPipe template '{}': {error}\nhelp: keep Vapor Client SteamPipe templates checked in under {STEAMPIPE_TEMPLATE_ROOT}",
            path.display()
        )
    })
}

fn render_template(template: &str, values: &[(&str, String)]) -> String {
    let mut rendered = template.to_owned();
    for (key, value) in values {
        rendered = rendered.replace(&format!("{{{{{key}}}}}"), value);
    }
    rendered
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
