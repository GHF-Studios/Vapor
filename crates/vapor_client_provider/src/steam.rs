//! SteamCMD-backed beta-branch SteamPipe publishing.

use crate::{
    discovery::{EnvironmentPaths, InstallationPaths, ensure_contained},
    distribution::{self, DistributionManifest, StageOptions, SteamDepotKind},
    manifest,
};
use std::{
    fs,
    hash::Hasher,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
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
    /// Fingerprint captured by a prior service-owned review.
    pub expected_fingerprint: Option<&'a str>,
}

/// Result of a SteamPipe publish or dry-run preview.
#[derive(Debug, Clone)]
pub struct PublishReport {
    script: PathBuf,
    stage: distribution::StageReport,
    fingerprint: String,
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

    /// Stable fingerprint for the exact staged depot paths and file bytes.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
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
    if branch == "default"
        || branch.trim() != branch
        || branch.is_empty()
        || branch.len() > 64
        || !branch
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    {
        return Err("automatic publishing requires a non-default beta branch".to_owned());
    }
    if !options.dry_run && !options.confirmed {
        return Err("publishing requires --yes after reviewing --dry-run".to_owned());
    }
    if !options.dry_run && options.expected_fingerprint.is_none() {
        return Err("publishing requires the fingerprint from a current service review".to_owned());
    }
    if options.account.trim().is_empty() || options.account.len() > 128 {
        return Err("Steamworks account must contain 1 to 128 bytes".to_owned());
    }
    if options.description.trim().is_empty() || options.description.len() > 256 {
        return Err("Steam build description must contain 1 to 256 bytes".to_owned());
    }
    let stage = distribution::stage_with_options(paths, manifest, options.stage_options.clone())?;
    smoke(&stage, &options.stage_options)?;
    let fingerprint = fingerprint_stage(&stage)?;
    if options
        .expected_fingerprint
        .is_some_and(|expected| expected != fingerprint)
    {
        return Err(format!(
            "staged Steam payload changed after review (expected {}, got {fingerprint}); stage and confirm the release again",
            options.expected_fingerprint.expect("checked above")
        ));
    }
    let script = write_build_script(
        paths,
        manifest,
        &stage,
        branch,
        options.description,
        options.dry_run,
    )?;
    if options.dry_run {
        return Ok(PublishReport {
            script,
            stage,
            fingerprint,
        });
    }
    let steamcmd = executable(paths)?;
    let output = Command::new(&steamcmd)
        .args(["+login", options.account, "+run_app_build"])
        .arg(&script)
        .arg("+quit")
        .current_dir(steamcmd.parent().expect("SteamCMD has a parent"))
        .stdin(Stdio::inherit())
        .output()
        .map_err(|error| format!("failed to start SteamCMD: {error}"))?;
    forward_steamcmd_output(&output.stdout, &output.stderr);
    if output.status.success() {
        Ok(PublishReport {
            script,
            stage,
            fingerprint,
        })
    } else {
        Err(format!("Steam publish exited with {}", output.status))
    }
}

fn forward_steamcmd_output(stdout: &[u8], stderr: &[u8]) {
    use std::io::Write;

    // The Client service reserves stdout for framed protocol messages.
    let mut log = std::io::stderr().lock();
    for output in [stdout, stderr] {
        if !output.is_empty() {
            let _ = log.write_all(output);
            if !output.ends_with(b"\n") {
                let _ = log.write_all(b"\n");
            }
        }
    }
    let _ = log.flush();
}

fn fingerprint_stage(stage: &distribution::StageReport) -> Result<String, String> {
    fingerprint_tree(stage.root())
}

fn fingerprint_tree(root: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let mut entries = fs::read_dir(&directory)
            .map_err(|error| format!("{}: {error}", directory.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries.into_iter().rev() {
            let path = entry.path();
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_dir() {
                pending.push(path);
            } else if kind.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|error| error.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push((relative, path));
            } else {
                return Err(format!(
                    "staged Steam payload contains an unsupported file type: {}",
                    path.display()
                ));
            }
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hash = Fnv64::default();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    for (path, source) in &files {
        hash.write(path.as_bytes());
        hash.write_u8(0);
        let mut file = fs::File::open(source).map_err(|error| error.to_string())?;
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|error| format!("{}: {error}", source.display()))?;
            if read == 0 {
                break;
            }
            hash.write(&buffer[..read]);
            bytes = bytes.saturating_add(read as u64);
        }
        hash.write_u8(0xff);
    }
    Ok(format!(
        "fnv1a64:{:016x}:{}:{bytes}",
        hash.finish(),
        files.len()
    ))
}

struct Fnv64(u64);

impl Default for Fnv64 {
    fn default() -> Self {
        Self(0xcbf29ce484222325)
    }
}

impl Hasher for Fnv64 {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
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
                    depot.root().join("bin").join(target).join("vapor-launcher"),
                    "staged Linux depot has no target Vapor Launcher binary",
                )?;
                require_file(
                    depot.root().join("bin").join(target).join("vapor-studio"),
                    "staged Linux depot has no target Vapor Studio binary",
                )?;
                require_file(
                    depot.root().join("bin").join(target).join("vapor-ide"),
                    "staged Linux depot has no target Vapor IDE binary",
                )?;
                require_file(
                    depot
                        .root()
                        .join("bin")
                        .join(target)
                        .join("vapor-installer-gui"),
                    "staged Linux depot has no target Vapor Installer GUI binary",
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
                require_file(
                    depot
                        .root()
                        .join("bin")
                        .join(target)
                        .join("libsteam_api.so"),
                    "staged Linux depot has no Steamworks runtime library",
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
                        .join("vapor-launcher.exe"),
                    "staged Windows depot has no target Vapor Launcher binary",
                )?;
                require_file(
                    depot
                        .root()
                        .join("bin")
                        .join(target)
                        .join("vapor-studio.exe"),
                    "staged Windows depot has no target Vapor Studio binary",
                )?;
                require_file(
                    depot.root().join("bin").join(target).join("vapor-ide.exe"),
                    "staged Windows depot has no target Vapor IDE binary",
                )?;
                require_file(
                    depot
                        .root()
                        .join("bin")
                        .join(target)
                        .join("vapor-installer-gui.exe"),
                    "staged Windows depot has no target Vapor Installer GUI binary",
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
                    depot
                        .root()
                        .join("bin")
                        .join(target)
                        .join("steam_api64.dll"),
                    "staged Windows depot has no Steamworks runtime library",
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

#[cfg(test)]
mod tests {
    use super::fingerprint_tree;
    use std::{fs, path::PathBuf};

    fn temporary_root(label: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock follows epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "vapor-steam-fingerprint-{label}-{}-{unique}",
            std::process::id()
        ))
    }

    #[test]
    fn reviewed_payload_fingerprint_is_stable_and_tracks_file_bytes() {
        let root = temporary_root("stable");
        fs::create_dir_all(root.join("nested")).expect("create test tree");
        fs::write(root.join("nested/payload.bin"), b"first payload").expect("write payload");
        let initial = fingerprint_tree(&root).expect("fingerprint staged tree");
        assert_eq!(
            fingerprint_tree(&root).expect("repeat fingerprint"),
            initial
        );

        fs::write(root.join("nested/payload.bin"), b"changed payload").expect("change payload");
        assert_ne!(
            fingerprint_tree(&root).expect("fingerprint changed tree"),
            initial
        );
        fs::remove_dir_all(root).expect("remove test tree");
    }
}
