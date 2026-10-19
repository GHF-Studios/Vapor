use std::fs;
use std::hash::Hasher;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::toolchain::toolchain_status;

use super::error::RootCommandError;
use super::types::{RootPackageReport, RootPackageRequest};

const ROOT_OUTPUT_DIR: &str = "root";
const CONTENT_DIR: &str = "content";
const SCRIPTS_DIR: &str = "scripts";
const STEAM_BUILD_DIR: &str = "steam-build";
const BIN_DIR: &str = "bin";
const ACTIVATION_SCRIPT_UNIX: [&str; 2] = ["vapor-env.sh", "vapor_env.sh"];
const ACTIVATION_SCRIPT_WINDOWS: [&str; 2] = ["vapor-env.cmd", "vapor_env.cmd"];

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
        let included_roots = package_roots(&status.vapor_home)?;

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
        let planned_fingerprint = request
            .plan
            .then(|| fingerprint_roots(&self.included_roots))
            .transpose()?;
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

        let fingerprint = if let Some(fingerprint) = planned_fingerprint {
            fingerprint
        } else {
            let staged_roots = self
                .included_roots
                .iter()
                .map(|root| {
                    PackageRoot::new(
                        self.content_root.join(&root.depot_path),
                        root.depot_path.clone(),
                    )
                })
                .collect::<Vec<_>>();
            fingerprint_roots(&staged_roots)?
        };

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
            fingerprint,
        })
    }
}

fn package_roots(vapor_home: &Path) -> Result<Vec<PackageRoot>, RootCommandError> {
    let bin = vapor_home.join(BIN_DIR);
    if !bin.is_dir() {
        return Err(RootCommandError::MissingPackageInput(bin));
    }

    // Keep the deployed target directories, runtime libraries, and launch
    // wrappers together. Flattening only the executable names both loses the
    // platform-specific layout and drops dynamic libraries needed at launch.
    let mut roots = vec![PackageRoot::new(bin, PathBuf::from(BIN_DIR))];
    let activation_script = activation_script_candidates()
        .iter()
        .map(|name| vapor_home.join(name))
        .find(|path| path.is_file())
        .ok_or_else(|| {
            RootCommandError::MissingPackageInput(
                vapor_home.join(activation_script_candidates()[0]),
            )
        })?;
    let activation_name = activation_script
        .file_name()
        .expect("activation script candidate has a filename")
        .to_owned();
    roots.push(PackageRoot::new(
        activation_script,
        PathBuf::from(activation_name),
    ));
    Ok(roots)
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
    copy_tree_with_ancestors(source, target, plan, &mut Vec::new())
}

fn copy_tree_with_ancestors(
    source: &Path,
    target: &Path,
    plan: bool,
    ancestors: &mut Vec<PathBuf>,
) -> Result<usize, RootCommandError> {
    let metadata = fs::symlink_metadata(source)?;

    if metadata.file_type().is_symlink() {
        let resolved = fs::canonicalize(source)?;
        if ancestors.contains(&resolved) {
            return Err(RootCommandError::SymlinkCycle(source.to_path_buf()));
        }
        return copy_tree_with_ancestors(&resolved, target, plan, ancestors);
    }

    if metadata.is_dir() {
        let canonical = fs::canonicalize(source)?;
        if ancestors.contains(&canonical) {
            return Err(RootCommandError::SymlinkCycle(source.to_path_buf()));
        }
        ancestors.push(canonical);
        if !plan {
            fs::create_dir_all(target)?;
        }

        let mut copied_files = 0;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copied_files += copy_tree_with_ancestors(
                &entry.path(),
                &target.join(entry.file_name()),
                plan,
                ancestors,
            )?;
        }
        ancestors.pop();
        return Ok(copied_files);
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

fn fingerprint_roots(roots: &[PackageRoot]) -> Result<String, RootCommandError> {
    let mut hash = Fnv64::default();
    let mut files = 0usize;
    for root in roots {
        hash.write(root.depot_path.to_string_lossy().as_bytes());
        hash.write_u8(0);
        files += fingerprint_tree(&root.source, &root.depot_path, &mut hash, &mut Vec::new())?;
    }
    Ok(format!("fnv1a64:{:016x}:{files}", hash.finish()))
}

fn fingerprint_tree(
    source: &Path,
    package_path: &Path,
    hash: &mut Fnv64,
    ancestors: &mut Vec<PathBuf>,
) -> Result<usize, RootCommandError> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        return fingerprint_tree(&fs::canonicalize(source)?, package_path, hash, ancestors);
    }
    if metadata.is_dir() {
        let canonical = fs::canonicalize(source)?;
        if ancestors.contains(&canonical) {
            return Err(RootCommandError::SymlinkCycle(source.to_path_buf()));
        }
        ancestors.push(canonical);
        let mut entries = fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        let mut files = 0;
        for entry in entries {
            files += fingerprint_tree(
                &entry.path(),
                &package_path.join(entry.file_name()),
                hash,
                ancestors,
            )?;
        }
        ancestors.pop();
        return Ok(files);
    }
    if !metadata.is_file() {
        return Ok(0);
    }

    hash.write(package_path.to_string_lossy().as_bytes());
    hash.write_u8(0);
    let mut file = fs::File::open(source)?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.write(&buffer[..read]);
    }
    hash.write_u8(0xff);
    Ok(1)
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

fn activation_script_candidates() -> &'static [&'static str] {
    if cfg!(windows) {
        &ACTIVATION_SCRIPT_WINDOWS
    } else {
        &ACTIVATION_SCRIPT_UNIX
    }
}

#[cfg(test)]
mod tests {
    use super::{PackageRoot, fingerprint_roots};
    use std::{fs, path::PathBuf, time::SystemTime};

    #[test]
    fn root_review_fingerprint_tracks_package_path_and_file_bytes() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "vapor-root-fingerprint-{}-{unique}",
            std::process::id()
        ));
        let source = root.join("source");
        fs::create_dir_all(source.join("bin")).expect("create test payload");
        fs::write(source.join("bin/vapor"), b"reviewed executable").expect("write payload");
        let packages = [PackageRoot::new(source.join("bin"), PathBuf::from("bin"))];
        let first = fingerprint_roots(&packages).expect("fingerprint package");
        assert_eq!(
            first,
            fingerprint_roots(&packages).expect("repeat fingerprint")
        );

        fs::write(source.join("bin/vapor"), b"changed executable").expect("change payload");
        let changed = fingerprint_roots(&packages).expect("fingerprint changed package");
        assert_ne!(first, changed);
        fs::remove_dir_all(root).expect("remove test payload");
    }

    #[cfg(unix)]
    #[test]
    fn root_package_copy_rejects_recursive_directory_symlinks() {
        use super::copy_tree;
        use crate::root::error::RootCommandError;
        use std::os::unix::fs::symlink;

        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "vapor-root-symlink-cycle-{}-{unique}",
            std::process::id()
        ));
        let source = root.join("source");
        fs::create_dir_all(source.join("nested")).expect("create source tree");
        fs::write(source.join("nested/payload"), b"payload").expect("write source file");
        symlink(&source, source.join("nested/loop")).expect("create recursive directory link");

        let roots = [PackageRoot::new(source.clone(), PathBuf::from("bin"))];
        assert!(matches!(
            fingerprint_roots(&roots),
            Err(RootCommandError::SymlinkCycle(_))
        ));
        let result = copy_tree(&source, &root.join("staged"), false);
        assert!(matches!(result, Err(RootCommandError::SymlinkCycle(_))));

        fs::remove_dir_all(root).expect("remove test package");
    }
}
