//! Vapor's independently installed per-user home.
//!
//! Steam product roots are separate local products and are passed to Vapor as
//! explicit context through `VAPOR_PRODUCT_ROOT`. They never identify or own
//! the Vapor installation.

use std::env;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const VAPOR_HOME_ENV: &str = "VAPOR_HOME";
pub const VAPOR_PRODUCT_ROOT_ENV: &str = "VAPOR_PRODUCT_ROOT";
pub const VAPOR_HOME_MANIFEST_FILE_NAME: &str = "Vapor-Home.toml";

const STATE_DIR: &str = "data/state";
const HOME_MARKER: &str = "schema = 1\n\n[vapor]\nid = \"ghf-studios/vapor\"\n";
const BUNDLE_MARKER: &str = "Vapor-Standalone.toml";
const BUNDLE_DIRS: &[&str] = &["bin", "docs", "examples", "metadata", "resources"];

/// Known historical Vapor paths and their standalone-home destinations.
/// Other non-product paths are also migrated by their existing top-level
/// names; only the Packagepack manifest, one primary executable, and declared
/// target runtime libraries stay.
const LEGACY_HOME_PATHS: &[(&str, &str)] = &[
    (".vapor", "data/legacy-vapor"),
    (
        "Vapor-Standalone.toml",
        "data/migration/Vapor-Standalone.toml",
    ),
    ("Superworkspace.vapor.toml", "Superworkspace.vapor.toml"),
    ("bin", "bin"),
    ("cache", "cache"),
    ("cargo-home", "tools/cargo/home"),
    ("content", "content"),
    ("config", "config"),
    ("docs", "docs"),
    ("examples", "examples"),
    ("metadata", "metadata"),
    ("output", "output"),
    ("resources", "resources"),
    ("rustup", "tools/rustup/bin"),
    ("rustup-home", "tools/rustup/home"),
    ("sources", "sources"),
    ("state", "data/state"),
    ("tools", "tools"),
];

const MIGRATION_TARGET_ROOTS: &[&str] = &[
    "Superworkspace.vapor.toml",
    "bin",
    "cache",
    "content",
    "config",
    "data",
    "docs",
    "examples",
    "metadata",
    "output",
    "resources",
    "sources",
    "state",
    "tools",
];

#[derive(Debug, Clone)]
pub struct LegacyHomeMigrationReport {
    pub product_root: PathBuf,
    pub vapor_home: PathBuf,
    pub migrated_paths: Vec<PathBuf>,
    pub source_product_manifest: PathBuf,
    pub retained_product_entries: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallationRootSource {
    Explicit,
    Environment,
    Executable,
}

impl fmt::Display for InstallationRootSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Explicit => "explicit Vapor home",
            Self::Environment => VAPOR_HOME_ENV,
            Self::Executable => "executable under Vapor home",
        })
    }
}

#[derive(Debug, Clone)]
pub struct VaporInstallation {
    pub root: PathBuf,
    pub root_source: InstallationRootSource,
}

impl VaporInstallation {
    /// Resolve a caller-selected Vapor home. The home must carry Vapor's own
    /// marker; an `App.vapor.toml` Steam product manifest is never sufficient.
    pub fn from_explicit_root(root: PathBuf) -> Result<Self, InstallationError> {
        let canonical_root = fs::canonicalize(&root).map_err(|source| InstallationError::Io {
            path: root.clone(),
            source,
        })?;
        validate_home_marker(&canonical_root)?;
        Ok(Self {
            root: canonical_root,
            root_source: InstallationRootSource::Explicit,
        })
    }

    /// Resolve the default home without creating it. The installer owns
    /// creation and marker installation.
    pub fn default_home_root() -> Result<PathBuf, InstallationError> {
        platform_home_root().ok_or(InstallationError::HomeUnavailable)
    }

    /// Create the canonical home marker and Vapor-owned directory skeleton.
    /// This is used by the standalone installer after it has selected its
    /// destination; ordinary discovery remains read-only.
    pub fn initialize_home(root: &Path) -> Result<Self, InstallationError> {
        fs::create_dir_all(root).map_err(|source| InstallationError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let canonical_root = fs::canonicalize(root).map_err(|source| InstallationError::Io {
            path: root.to_path_buf(),
            source,
        })?;

        if canonical_root.join("App.vapor.toml").is_file()
            && !canonical_root.join(VAPOR_HOME_MANIFEST_FILE_NAME).is_file()
        {
            return Err(InstallationError::InvalidExplicitRoot {
                root: canonical_root,
                reason: "a Steam product root cannot be initialized as Vapor home",
            });
        }

        let marker = canonical_root.join(VAPOR_HOME_MANIFEST_FILE_NAME);
        if !marker.exists() {
            validate_home_adoption_entries(&canonical_root)?;
            fs::write(&marker, HOME_MARKER).map_err(|source| InstallationError::Io {
                path: marker.clone(),
                source,
            })?;
        }
        validate_home_marker(&canonical_root)?;

        for directory in [
            "bin",
            "config",
            "content",
            "data",
            "docs",
            "examples",
            "metadata",
            "output",
            "resources",
            "sources",
            "cache/build",
            "tools/cargo/bin",
            "tools/cargo/home",
            "tools/rustup/bin",
            "tools/rustup/home",
        ] {
            let path = canonical_root.join(directory);
            fs::create_dir_all(&path).map_err(|source| InstallationError::Io { path, source })?;
        }

        Ok(Self {
            root: canonical_root,
            root_source: InstallationRootSource::Explicit,
        })
    }

    /// Install the current platform's standalone distribution payload into a
    /// separate Vapor home. The bundle marker is required, and only the
    /// runtime-owned release directories are copied; user data, source
    /// checkouts, caches, and tools are created or managed by their own flows.
    pub fn install_from_bundle(
        bundle_root: &Path,
        home_root: &Path,
    ) -> Result<Self, InstallationError> {
        let bundle_root =
            fs::canonicalize(bundle_root).map_err(|source| InstallationError::Io {
                path: bundle_root.to_path_buf(),
                source,
            })?;
        if !bundle_root.is_dir() {
            return Err(InstallationError::InvalidBundle {
                root: bundle_root,
                reason: "bundle root is not a directory",
            });
        }
        validate_bundle(&bundle_root)?;

        let home_exists = home_root.exists();
        if !home_exists {
            fs::create_dir_all(home_root).map_err(|source| InstallationError::Io {
                path: home_root.to_path_buf(),
                source,
            })?;
        }
        let canonical_home =
            fs::canonicalize(home_root).map_err(|source| InstallationError::Io {
                path: home_root.to_path_buf(),
                source,
            })?;

        if canonical_home.join("App.vapor.toml").is_file()
            && !canonical_home.join(VAPOR_HOME_MANIFEST_FILE_NAME).is_file()
        {
            return Err(InstallationError::InvalidExplicitRoot {
                root: canonical_home,
                reason: "a Steam product root cannot be initialized as Vapor home",
            });
        }

        let home_marker = canonical_home.join(VAPOR_HOME_MANIFEST_FILE_NAME);
        if home_marker.is_file() {
            validate_home_marker(&canonical_home)?;
        } else {
            validate_home_adoption_entries(&canonical_home)?;
        }

        if canonical_home == bundle_root {
            return Self::initialize_home(&canonical_home);
        }
        if canonical_home.starts_with(&bundle_root) || bundle_root.starts_with(&canonical_home) {
            return Err(InstallationError::InvalidBundle {
                root: bundle_root,
                reason: "bundle and destination Vapor home must be separate directories",
            });
        }

        replace_bundle_payload(&bundle_root, &canonical_home)?;

        Self::initialize_home(&canonical_home)
    }

    /// Validate a standalone bundle before a first-run migration or install
    /// changes any user data.
    pub fn validate_standalone_bundle(bundle_root: &Path) -> Result<(), InstallationError> {
        let bundle_root =
            fs::canonicalize(bundle_root).map_err(|source| InstallationError::Io {
                path: bundle_root.to_path_buf(),
                source,
            })?;
        if !bundle_root.is_dir() {
            return Err(InstallationError::InvalidBundle {
                root: bundle_root,
                reason: "bundle root is not a directory",
            });
        }
        validate_bundle(&bundle_root)
    }

    /// Validate the current Packagepack mount or identify a former Steam
    /// product layout. Current mounts retain only their manifest, primary
    /// executable, and declared runtime libraries; old layouts are migrated.
    pub fn legacy_product_entries(product_root: &Path) -> Result<Vec<PathBuf>, InstallationError> {
        let product_root =
            fs::canonicalize(product_root).map_err(|source| InstallationError::Io {
                path: product_root.to_path_buf(),
                source,
            })?;
        let package_manifest = product_root.join("Packagepack.vapor.toml");
        if package_manifest.is_file() {
            let runtime_libraries = validate_packagepack_depot(&package_manifest, &product_root)?;
            let executable = packagepack_executable_name();
            let binary = product_root.join(executable);
            let metadata =
                fs::symlink_metadata(&binary).map_err(|source| InstallationError::Io {
                    path: binary.clone(),
                    source,
                })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(InstallationError::InvalidLegacyRoot {
                    root: product_root,
                    reason:
                        "the Loo Cast Steam depot must contain its platform Packagepack executable",
                });
            }
            let mut retained = vec![
                PathBuf::from("Packagepack.vapor.toml"),
                PathBuf::from(executable),
            ];
            retained.extend(runtime_libraries);
            return Ok(retained);
        }
        let app_manifest = product_root.join("App.vapor.toml");
        if !fs::symlink_metadata(&app_manifest)
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        {
            return Err(InstallationError::InvalidLegacyRoot {
                root: product_root,
                reason:
                    "App.vapor.toml is missing; refusing to treat this as the Loo Cast product root",
            });
        }
        let mut candidates = Vec::new();
        for entry in fs::read_dir(&product_root).map_err(|source| InstallationError::Io {
            path: product_root.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| InstallationError::Io {
                path: product_root.clone(),
                source,
            })?;
            let name = entry.file_name();
            if name == std::ffi::OsStr::new("App.vapor.toml")
                || LEGACY_HOME_PATHS
                    .iter()
                    .any(|(legacy, _)| name == std::ffi::OsStr::new(legacy))
            {
                continue;
            }
            let path = entry.path();
            if is_loo_cast_depot_product_root(&path)? {
                candidates.push(PathBuf::from(name));
            }
        }
        candidates.sort();
        if candidates.len() != 3 {
            if candidates.is_empty() {
                validate_legacy_vapor_client_manifest(&app_manifest, &product_root)?;
                // This is the known standalone-client Steam layout from before
                // Packagepack became the only depot product. Its top-level
                // entries belong to Vapor and are all moved into Vapor home.
                return Ok(Vec::new());
            }
            return Err(InstallationError::InvalidLegacyRoot {
                root: product_root,
                reason: "could not identify exactly three local depot product roots; update the Steam app manifest and product manifests first",
            });
        }
        let retained = candidates
            .iter()
            .map(|name| {
                let metadata = fs::symlink_metadata(product_root.join(name)).map_err(|source| {
                    InstallationError::Io {
                        path: product_root.join(name),
                        source,
                    }
                })?;
                Ok((name.as_os_str().to_os_string(), metadata.is_dir()))
            })
            .collect::<Result<Vec<_>, InstallationError>>()?;
        validate_depot_product_manifest(&app_manifest, &product_root, &retained)?;
        // Historical depot products are no longer retained in Steam. Their
        // validated roots are migrated into Vapor home along with old runtime
        // files; only the new packagepack-only depot layout stays mounted.
        Ok(Vec::new())
    }

    /// List every top-level entry that will leave the Steam root during the
    /// one-time move. Current Packagepack files are retained only when present
    /// in `retained_product_entries`; old app manifests, separate product
    /// roots, locator files, and Vapor tools move into Vapor home.
    pub fn legacy_entries_to_move(
        product_root: &Path,
        retained_product_entries: &[PathBuf],
    ) -> Result<Vec<PathBuf>, InstallationError> {
        let product_root =
            fs::canonicalize(product_root).map_err(|source| InstallationError::Io {
                path: product_root.to_path_buf(),
                source,
            })?;
        let retained = retained_product_entries
            .iter()
            .map(|path| validate_retained_product_entry(&product_root, path).map(|(name, _)| name))
            .collect::<Result<Vec<_>, _>>()?;
        let mut moving = Vec::new();
        for entry in fs::read_dir(&product_root).map_err(|source| InstallationError::Io {
            path: product_root.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| InstallationError::Io {
                path: product_root.clone(),
                source,
            })?;
            let name = entry.file_name();
            if retained.iter().any(|product| product == &name) {
                continue;
            }
            moving.push(PathBuf::from(name));
        }
        moving.sort();
        Ok(moving)
    }

    /// Move the known historical Vapor layout out of a Loo Cast Steam
    /// product root. The operation copies and validates the merged home before
    /// detaching the original directories; a failed detach keeps a recoverable
    /// copy in the Steam root while the complete home remains usable.
    ///
    /// Every top-level entry except the Packagepack manifest, primary
    /// executable, and manifest-declared host runtime libraries moves into
    /// Vapor home. Historical product roots and the old locator are migrated
    /// as legacy content. Known historical paths receive destinations; other
    /// legacy entries retain their names so no old Vapor-owned data is
    /// silently stranded in the Steam app.
    pub fn migrate_legacy_product_root(
        product_root: &Path,
        home_root: &Path,
        retained_product_entries: &[PathBuf],
    ) -> Result<LegacyHomeMigrationReport, InstallationError> {
        let product_root =
            fs::canonicalize(product_root).map_err(|source| InstallationError::Io {
                path: product_root.to_path_buf(),
                source,
            })?;
        let package_manifest = product_root.join("Packagepack.vapor.toml");
        let legacy_app_manifest = product_root.join("App.vapor.toml");
        let current_packagepack = package_manifest.is_file();
        let manifest_for_report = if current_packagepack {
            &package_manifest
        } else {
            &legacy_app_manifest
        };
        let packagepack_runtime_libraries = if current_packagepack {
            validate_packagepack_depot(&package_manifest, &product_root)?
        } else {
            Vec::new()
        };
        if !current_packagepack {
            if !fs::symlink_metadata(&legacy_app_manifest)
                .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
            {
                return Err(InstallationError::InvalidLegacyRoot {
                    root: product_root,
                    reason: "neither the current Packagepack depot nor a recognized legacy Loo Cast manifest was found",
                });
            }
        }

        fs::create_dir_all(home_root).map_err(|source| InstallationError::Io {
            path: home_root.to_path_buf(),
            source,
        })?;
        let home_root = fs::canonicalize(home_root).map_err(|source| InstallationError::Io {
            path: home_root.to_path_buf(),
            source,
        })?;
        if home_root == product_root
            || home_root.starts_with(&product_root)
            || product_root.starts_with(&home_root)
        {
            return Err(InstallationError::InvalidLegacyRoot {
                root: product_root,
                reason: "the Loo Cast product root and Vapor home must be separate, non-nested directories",
            });
        }

        let retained_entries = retained_product_entries
            .iter()
            .map(|path| validate_retained_product_entry(&product_root, path))
            .collect::<Result<Vec<_>, _>>()?;
        if current_packagepack {
            if retained_entries.len() != 2 + packagepack_runtime_libraries.len()
                || retained_entries.iter().any(|entry| entry.1)
                || !retained_entries
                    .iter()
                    .any(|entry| entry.0 == std::ffi::OsString::from("Packagepack.vapor.toml"))
                || !retained_entries
                    .iter()
                    .any(|entry| entry.0 == std::ffi::OsString::from(packagepack_executable_name()))
                || packagepack_runtime_libraries.iter().any(|library| {
                    !retained_entries
                        .iter()
                        .any(|entry| entry.0 == library.as_os_str())
                })
            {
                return Err(InstallationError::InvalidLegacyRoot {
                    root: product_root,
                    reason: "only the Packagepack manifest, target executable, and manifest-declared runtime libraries may remain in the Steam root",
                });
            }
        } else if !retained_entries.is_empty() {
            return Err(InstallationError::InvalidLegacyRoot {
                root: product_root,
                reason: "legacy Steam product folders must migrate into Vapor home",
            });
        }
        let mut retained_names = Vec::with_capacity(retained_entries.len() + 1);
        for (index, (name, _)) in retained_entries.iter().enumerate() {
            if retained_entries[..index]
                .iter()
                .any(|(prior, _)| prior == name)
            {
                return Err(InstallationError::InvalidLegacyRoot {
                    root: product_root,
                    reason: "a retained Steam product entry was declared more than once",
                });
            }
            retained_names.push(name.clone());
        }
        if current_packagepack {
            retained_names.push(std::ffi::OsString::from("Packagepack.vapor.toml"));
            validate_packagepack_depot(&package_manifest, &product_root)?;
        } else {
            let mut legacy_products = Vec::new();
            for entry in fs::read_dir(&product_root).map_err(|source| InstallationError::Io {
                path: product_root.clone(),
                source,
            })? {
                let entry = entry.map_err(|source| InstallationError::Io {
                    path: product_root.clone(),
                    source,
                })?;
                if is_loo_cast_depot_product_root(&entry.path())? {
                    let metadata = entry.metadata().map_err(|source| InstallationError::Io {
                        path: entry.path(),
                        source,
                    })?;
                    legacy_products.push((entry.file_name(), metadata.is_dir()));
                }
            }
            if legacy_products.is_empty() {
                validate_legacy_vapor_client_manifest(&legacy_app_manifest, &product_root)?;
            } else {
                validate_depot_product_manifest(
                    &legacy_app_manifest,
                    &product_root,
                    &legacy_products,
                )?;
            }
        }
        let mut additional_legacy_roots = Vec::new();
        for entry in fs::read_dir(&product_root).map_err(|source| InstallationError::Io {
            path: product_root.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| InstallationError::Io {
                path: product_root.clone(),
                source,
            })?;
            let name = entry.file_name();
            if !LEGACY_HOME_PATHS
                .iter()
                .any(|(legacy, _)| name == std::ffi::OsStr::new(legacy))
                && !retained_names
                    .iter()
                    .any(|retained| name.as_os_str() == retained.as_os_str())
            {
                additional_legacy_roots.push(PathBuf::from(name));
            }
        }
        let mut migration_roots = MIGRATION_TARGET_ROOTS
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        for name in &additional_legacy_roots {
            if !migration_roots.iter().any(|root| root == name) {
                migration_roots.push(name.clone());
            }
        }

        let marker = home_root.join(VAPOR_HOME_MANIFEST_FILE_NAME);
        if marker.is_file() {
            validate_home_marker(&home_root)?;
        } else {
            if home_root.join("App.vapor.toml").is_file() {
                return Err(InstallationError::InvalidExplicitRoot {
                    root: home_root,
                    reason: "a Steam product root cannot be initialized as Vapor home",
                });
            }
            validate_home_adoption_entries(&home_root)?;
        }
        // Initialize before creating transaction directories. If initialization
        // fails, every legacy path is still in place in the Steam root.
        Self::initialize_home(&home_root)?;

        let transaction_id = format!("{}-{}", std::process::id(), migration_nonce());
        let staging = home_root.join(format!(".vapor-migration-staging-{transaction_id}"));
        let previous = home_root.join(format!(".vapor-migration-previous-{transaction_id}"));
        let detached = product_root.join(format!(".vapor-migrated-{transaction_id}"));
        for (root, prefix) in [
            (&home_root, ".vapor-migration-staging-"),
            (&home_root, ".vapor-migration-previous-"),
            (&product_root, ".vapor-migrated-"),
        ] {
            if let Some(path) = find_prefixed_entry(root, prefix)? {
                return Err(InstallationError::MigrationRecoveryRequired { path });
            }
        }
        for path in [&staging, &previous, &detached] {
            if path.exists() {
                return Err(InstallationError::MigrationRecoveryRequired { path: path.clone() });
            }
        }
        fs::create_dir(&staging).map_err(|source| InstallationError::Io {
            path: staging.clone(),
            source,
        })?;
        if let Err(source) = fs::create_dir(&previous) {
            let _ = fs::remove_dir(&staging);
            return Err(InstallationError::Io {
                path: previous,
                source,
            });
        }

        let mut legacy_vapor_destinations = Vec::new();
        let stage_result = (|| {
            for root in &migration_roots {
                let current = home_root.join(root);
                if current.exists() {
                    merge_regular_tree(&current, &staging.join(root))?;
                }
            }
            for (source_name, target_name) in LEGACY_HOME_PATHS {
                let source = product_root.join(source_name);
                match fs::symlink_metadata(&source) {
                    Ok(_) if *source_name == "bin" => {
                        merge_legacy_bin(&source, &staging.join(target_name), Path::new("bin"))?;
                    }
                    Ok(_) if *source_name == ".vapor" => {
                        legacy_vapor_destinations = merge_legacy_vapor(&source, &staging)?;
                    }
                    Ok(_) => merge_regular_tree(&source, &staging.join(target_name))?,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(source_error) => {
                        return Err(InstallationError::Io {
                            path: source,
                            source: source_error,
                        });
                    }
                }
            }
            for root in &additional_legacy_roots {
                merge_regular_tree(&product_root.join(root), &staging.join(root))?;
            }
            Ok(())
        })();
        if let Err(error) = stage_result {
            let _ = fs::remove_dir_all(&staging);
            let _ = fs::remove_dir(&previous);
            return Err(error);
        }

        let mut backed_up: Vec<PathBuf> = Vec::new();
        let mut promoted: Vec<PathBuf> = Vec::new();
        let promotion = (|| {
            for root in &migration_roots {
                let staged = staging.join(root);
                if !staged.exists() {
                    continue;
                }
                let target = home_root.join(root);
                if target.exists() {
                    let backup = previous.join(root);
                    fs::rename(&target, &backup).map_err(|source| InstallationError::Io {
                        path: target.clone(),
                        source,
                    })?;
                    backed_up.push(root.clone());
                }
                fs::rename(&staged, &target).map_err(|source| InstallationError::Io {
                    path: target.clone(),
                    source,
                })?;
                promoted.push(root.clone());
            }
            Ok(())
        })();
        if let Err(error) = promotion {
            let rollback =
                rollback_home_migration(&home_root, &staging, &previous, &promoted, &backed_up);
            let _ = fs::remove_dir_all(&staging);
            if rollback.is_empty() {
                let _ = fs::remove_dir_all(&previous);
                return Err(error);
            }
            return Err(InstallationError::TransactionRollback {
                root: home_root,
                cause: error.to_string(),
                rollback: rollback.join("; "),
            });
        }

        let mut detached_paths: Vec<PathBuf> = Vec::new();
        if let Err(error) = fs::create_dir(&detached) {
            let rollback =
                rollback_home_migration(&home_root, &staging, &previous, &promoted, &backed_up);
            let _ = fs::remove_dir_all(&staging);
            if rollback.is_empty() {
                let _ = fs::remove_dir_all(&previous);
                return Err(InstallationError::Io {
                    path: detached,
                    source: error,
                });
            }
            return Err(InstallationError::TransactionRollback {
                root: home_root,
                cause: format!("failed to prepare source detachment: {error}"),
                rollback: rollback.join("; "),
            });
        }
        for (source_name, _) in LEGACY_HOME_PATHS {
            let source = product_root.join(source_name);
            if !source.exists() {
                continue;
            }
            let target = detached.join(source_name);
            let detach_result = if *source_name == "bin" {
                detached_paths.push(PathBuf::from(source_name));
                detach_legacy_bin(&source, &target, Path::new("bin"))
            } else {
                fs::rename(&source, &target).map(|_| true)
            };
            if detach_result.as_ref().is_ok_and(|moved| !*moved) {
                detached_paths.pop();
            } else if detach_result.is_ok() && *source_name != "bin" {
                detached_paths.push(PathBuf::from(source_name));
            }
            if let Err(error) = detach_result {
                let mut rollback = Vec::new();
                for name in detached_paths.iter().rev() {
                    let from = detached.join(name);
                    let to = product_root.join(name);
                    let restore_result = if name == Path::new("bin") {
                        restore_migrated_bin(&from, &to)
                    } else {
                        fs::rename(&from, &to)
                    };
                    if let Err(restore_error) = restore_result {
                        rollback.push(format!("restore {}: {restore_error}", to.display()));
                    }
                }
                rollback.extend(rollback_home_migration(
                    &home_root, &staging, &previous, &promoted, &backed_up,
                ));
                if rollback.is_empty() {
                    let _ = fs::remove_dir_all(&detached);
                    let _ = fs::remove_dir_all(&staging);
                    let _ = fs::remove_dir_all(&previous);
                    return Err(InstallationError::Io {
                        path: source,
                        source: error,
                    });
                }
                return Err(InstallationError::TransactionRollback {
                    root: product_root,
                    cause: format!("failed to detach migrated path: {error}"),
                    rollback: rollback.join("; "),
                });
            }
        }
        for name in &additional_legacy_roots {
            let source = product_root.join(name);
            let target = detached.join(name);
            if let Err(error) = fs::rename(&source, &target) {
                let mut rollback = Vec::new();
                for detached_name in detached_paths.iter().rev() {
                    let from = detached.join(detached_name);
                    let to = product_root.join(detached_name);
                    let restore_result = if detached_name == Path::new("bin") {
                        restore_migrated_bin(&from, &to)
                    } else {
                        fs::rename(&from, &to)
                    };
                    if let Err(restore_error) = restore_result {
                        rollback.push(format!("restore {}: {restore_error}", to.display()));
                    }
                }
                rollback.extend(rollback_home_migration(
                    &home_root, &staging, &previous, &promoted, &backed_up,
                ));
                if rollback.is_empty() {
                    let _ = fs::remove_dir_all(&detached);
                    let _ = fs::remove_dir_all(&staging);
                    let _ = fs::remove_dir_all(&previous);
                    return Err(InstallationError::Io {
                        path: source,
                        source: error,
                    });
                }
                return Err(InstallationError::TransactionRollback {
                    root: product_root,
                    cause: format!("failed to detach migrated path: {error}"),
                    rollback: rollback.join("; "),
                });
            }
            detached_paths.push(name.clone());
        }

        let mut migrated_paths = Vec::new();
        for name in &detached_paths {
            if name == Path::new(".vapor") {
                migrated_paths.extend(
                    legacy_vapor_destinations
                        .iter()
                        .map(|destination| home_root.join(destination)),
                );
                continue;
            }
            if let Some((_, target)) = LEGACY_HOME_PATHS
                .iter()
                .find(|(source, _)| Path::new(*source) == name.as_path())
            {
                migrated_paths.push(home_root.join(target));
            } else if additional_legacy_roots.iter().any(|root| root == name) {
                migrated_paths.push(home_root.join(name));
            }
        }
        migrated_paths.sort();
        migrated_paths.dedup();
        if let Err(error) = fs::remove_dir_all(&detached) {
            return Err(InstallationError::MigrationCleanupRequired {
                home: home_root,
                recovery: detached,
                detail: format!("migration completed; Steam-root cleanup failed: {error}"),
            });
        }
        if let Err(error) = fs::remove_dir_all(&previous) {
            return Err(InstallationError::MigrationCleanupRequired {
                home: home_root,
                recovery: previous,
                detail: format!("migration completed; old-home backup cleanup failed: {error}"),
            });
        }
        let _ = fs::remove_dir_all(&staging);

        Ok(LegacyHomeMigrationReport {
            product_root: product_root.clone(),
            vapor_home: home_root,
            migrated_paths,
            source_product_manifest: manifest_for_report.clone(),
            retained_product_entries: retained_product_entries
                .iter()
                .map(|path| product_root.join(path))
                .collect(),
        })
    }

    /// Discover the active standalone Vapor home from inherited context or the
    /// location of the running Vapor binary. There is no Steam-app fallback.
    pub fn discover() -> Result<Self, InstallationError> {
        if let Some(root) = explicit_vapor_home() {
            let mut installation = Self::from_explicit_root(root)?;
            installation.root_source = InstallationRootSource::Environment;
            return Ok(installation);
        }

        let executable = env::current_exe().map_err(InstallationError::CurrentExecutable)?;
        let root =
            home_root_from_executable(&executable).ok_or_else(|| InstallationError::NotFound {
                executable: executable.clone(),
            })?;
        let mut installation = Self::from_explicit_root(root)?;
        installation.root_source = InstallationRootSource::Executable;
        Ok(installation)
    }

    /// Vapor owns its complete mutable state tree inside this independent home.
    pub fn user_data_root(&self) -> PathBuf {
        self.root.clone()
    }

    /// Canonical persisted state path. No Steam-root or hidden `.vapor`
    /// fallback is consulted.
    pub fn state_root(&self) -> PathBuf {
        self.root.join(STATE_DIR)
    }

    /// Ensure the canonical persisted state directory exists.
    pub fn ensure_state_root(&self) -> Result<PathBuf, InstallationError> {
        let state_root = self.state_root();
        fs::create_dir_all(&state_root).map_err(|source| InstallationError::Io {
            path: state_root.clone(),
            source,
        })?;
        Ok(state_root)
    }
}

fn validate_home_marker(root: &Path) -> Result<(), InstallationError> {
    let marker = root.join(VAPOR_HOME_MANIFEST_FILE_NAME);
    let source = fs::read_to_string(&marker).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            InstallationError::InvalidExplicitRoot {
                root: root.to_path_buf(),
                reason: "Vapor-Home.toml is missing",
            }
        } else {
            InstallationError::Io {
                path: marker.clone(),
                source: error,
            }
        }
    })?;
    let manifest: toml::Value =
        toml::from_str(&source).map_err(|_| InstallationError::InvalidExplicitRoot {
            root: root.to_path_buf(),
            reason: "Vapor-Home.toml is not valid TOML",
        })?;
    let manifest_identity = manifest
        .get("vapor")
        .and_then(|vapor| vapor.get("id"))
        .and_then(toml::Value::as_str);
    if manifest.get("schema").and_then(toml::Value::as_integer) != Some(1)
        || manifest_identity != Some("ghf-studios/vapor")
    {
        return Err(InstallationError::InvalidExplicitRoot {
            root: root.to_path_buf(),
            reason: "Vapor-Home.toml does not identify the supported Vapor home",
        });
    }
    Ok(())
}

fn validate_home_adoption_entries(root: &Path) -> Result<(), InstallationError> {
    const ALLOWED_ENTRIES: &[&str] = &[
        "App.vapor.toml",
        "Superworkspace.vapor.toml",
        "Vapor-Standalone.toml",
        "bin",
        "cache",
        "config",
        "content",
        "data",
        "docs",
        "examples",
        "metadata",
        "output",
        "resources",
        "sources",
        "tools",
    ];

    let entries = fs::read_dir(root).map_err(|source| InstallationError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| InstallationError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let name = entry.file_name();
        if !ALLOWED_ENTRIES
            .iter()
            .any(|allowed| name == std::ffi::OsStr::new(allowed))
        {
            return Err(InstallationError::InvalidExplicitRoot {
                root: root.to_path_buf(),
                reason: "destination contains files not recognized as Vapor-owned home data",
            });
        }
    }
    Ok(())
}

fn validate_bundle(root: &Path) -> Result<(), InstallationError> {
    let marker = root.join(BUNDLE_MARKER);
    let source = fs::read_to_string(&marker).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            InstallationError::InvalidBundle {
                root: root.to_path_buf(),
                reason: "Vapor-Standalone.toml is missing",
            }
        } else {
            InstallationError::Io {
                path: marker.clone(),
                source: error,
            }
        }
    })?;
    let manifest: toml::Value =
        toml::from_str(&source).map_err(|_| InstallationError::InvalidBundle {
            root: root.to_path_buf(),
            reason: "Vapor-Standalone.toml is not valid TOML",
        })?;
    let manifest_identity = manifest
        .get("vapor")
        .and_then(|vapor| vapor.get("id"))
        .and_then(toml::Value::as_str);
    if manifest.get("schema").and_then(toml::Value::as_integer) != Some(1)
        || manifest_identity != Some("ghf-studios/vapor")
    {
        return Err(InstallationError::InvalidBundle {
            root: root.to_path_buf(),
            reason: "Vapor-Standalone.toml does not identify the supported release",
        });
    }
    for directory in BUNDLE_DIRS {
        if !root.join(directory).is_dir() {
            return Err(InstallationError::InvalidBundle {
                root: root.to_path_buf(),
                reason: "a required release directory is missing",
            });
        }
    }
    let host_target = crate::HostTarget::current()
        .map_err(|_| InstallationError::InvalidBundle {
            root: root.to_path_buf(),
            reason: "this platform is not a supported Vapor release target",
        })?
        .triple();
    let payload_target = manifest
        .get("payload")
        .and_then(|payload| payload.get("target"))
        .and_then(toml::Value::as_str);
    let declared_targets = manifest
        .get("payload")
        .and_then(|payload| payload.get("targets"))
        .and_then(toml::Value::as_array);
    if payload_target != Some(host_target)
        || !declared_targets.is_some_and(|targets| {
            targets
                .iter()
                .any(|target| target.as_str() == Some(host_target))
        })
    {
        return Err(InstallationError::InvalidBundle {
            root: root.to_path_buf(),
            reason: "the standalone bundle targets a different ABI",
        });
    }
    for executable in [
        "vapor",
        "vapor-client-service",
        "vapor-gui",
        "vapor-installer-gui",
    ] {
        let path = root
            .join("bin")
            .join(host_target)
            .join(format!("{executable}{}", env::consts::EXE_SUFFIX));
        if !path.is_file() {
            return Err(InstallationError::InvalidBundle {
                root: root.to_path_buf(),
                reason: "the current platform runtime payload is incomplete",
            });
        }
    }
    Ok(())
}

fn replace_bundle_payload(bundle_root: &Path, home_root: &Path) -> Result<(), InstallationError> {
    let transaction_id = std::process::id();
    let staging = home_root.join(format!(".vapor-installing-{transaction_id}"));
    let previous = home_root.join(format!(".vapor-previous-{transaction_id}"));
    if staging.exists() || previous.exists() {
        return Err(InstallationError::InvalidBundle {
            root: bundle_root.to_path_buf(),
            reason: "an interrupted bundle installation needs recovery before retrying",
        });
    }
    fs::create_dir(&staging).map_err(|source| InstallationError::Io {
        path: staging.clone(),
        source,
    })?;
    if let Err(source) = fs::create_dir(&previous) {
        let _ = fs::remove_dir(&staging);
        return Err(InstallationError::Io {
            path: previous,
            source,
        });
    }

    let stage_result = (|| {
        for name in BUNDLE_DIRS {
            let source = bundle_root.join(name);
            if !source.is_dir() {
                return Err(InstallationError::InvalidBundle {
                    root: bundle_root.to_path_buf(),
                    reason: "a required release directory is missing",
                });
            }
            copy_regular_tree(&source, &staging.join(name))?;
        }
        Ok(())
    })();
    if let Err(error) = stage_result {
        let _ = fs::remove_dir_all(&staging);
        let _ = fs::remove_dir(&previous);
        return Err(error);
    }

    let mut backed_up = Vec::new();
    let mut installed = Vec::new();
    let promotion = (|| {
        for name in BUNDLE_DIRS {
            let target = home_root.join(name);
            match fs::symlink_metadata(&target) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    return Err(InstallationError::InvalidExplicitRoot {
                        root: home_root.to_path_buf(),
                        reason:
                            "a release directory is occupied by a non-directory or symbolic link",
                    });
                }
                Ok(_) => {
                    let backup = previous.join(name);
                    fs::rename(&target, &backup).map_err(|source| InstallationError::Io {
                        path: target.clone(),
                        source,
                    })?;
                    backed_up.push(name.to_string());
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(InstallationError::Io {
                        path: target,
                        source,
                    });
                }
            }
            fs::rename(staging.join(name), &target).map_err(|source| InstallationError::Io {
                path: target.clone(),
                source,
            })?;
            installed.push(name.to_string());
        }
        Ok(())
    })();

    if let Err(error) = promotion {
        let mut rollback_failures = Vec::new();
        for name in installed.iter().rev() {
            let target = home_root.join(name);
            if let Err(source) = fs::remove_dir_all(&target) {
                rollback_failures.push(format!("remove {}: {source}", target.display()));
            }
        }
        for name in backed_up.iter().rev() {
            let backup = previous.join(name);
            let target = home_root.join(name);
            if let Err(source) = fs::rename(&backup, &target) {
                rollback_failures.push(format!("restore {}: {source}", target.display()));
            }
        }
        let _ = fs::remove_dir_all(&staging);
        if rollback_failures.is_empty() {
            let _ = fs::remove_dir_all(&previous);
            return Err(error);
        }
        return Err(InstallationError::TransactionRollback {
            root: home_root.to_path_buf(),
            cause: error.to_string(),
            rollback: rollback_failures.join("; "),
        });
    }

    fs::remove_dir_all(&staging).map_err(|source| InstallationError::Io {
        path: staging,
        source,
    })?;
    fs::remove_dir_all(&previous).map_err(|source| InstallationError::Io {
        path: previous,
        source,
    })?;
    Ok(())
}

fn copy_regular_tree(source: &Path, target: &Path) -> Result<(), InstallationError> {
    let metadata = fs::symlink_metadata(source).map_err(|source_error| InstallationError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(InstallationError::InvalidBundle {
            root: source.to_path_buf(),
            reason: "release payload contains a symbolic link",
        });
    }
    if metadata.is_dir() {
        fs::create_dir_all(target).map_err(|source| InstallationError::Io {
            path: target.to_path_buf(),
            source,
        })?;
        for entry in fs::read_dir(source).map_err(|source_error| InstallationError::Io {
            path: source.to_path_buf(),
            source: source_error,
        })? {
            let entry = entry.map_err(|source_error| InstallationError::Io {
                path: source.to_path_buf(),
                source: source_error,
            })?;
            copy_regular_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if metadata.is_file() {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| InstallationError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::copy(source, target).map_err(|source_error| InstallationError::Io {
            path: source.to_path_buf(),
            source: source_error,
        })?;
    }
    Ok(())
}

fn merge_regular_tree(source: &Path, target: &Path) -> Result<(), InstallationError> {
    let source_metadata =
        fs::symlink_metadata(source).map_err(|source_error| InstallationError::Io {
            path: source.to_path_buf(),
            source: source_error,
        })?;
    if source_metadata.file_type().is_symlink() {
        return Err(InstallationError::InvalidLegacyRoot {
            root: source.to_path_buf(),
            reason: "a Vapor-owned source contains a symbolic link; refusing to migrate it",
        });
    }

    if source_metadata.is_dir() {
        match fs::symlink_metadata(target) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(InstallationError::MigrationConflict {
                    path: target.to_path_buf(),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir_all(target).map_err(|source| InstallationError::Io {
                    path: target.to_path_buf(),
                    source,
                })?;
            }
            Err(source_error) => {
                return Err(InstallationError::Io {
                    path: target.to_path_buf(),
                    source: source_error,
                });
            }
        }
        for entry in fs::read_dir(source).map_err(|source_error| InstallationError::Io {
            path: source.to_path_buf(),
            source: source_error,
        })? {
            let entry = entry.map_err(|source_error| InstallationError::Io {
                path: source.to_path_buf(),
                source: source_error,
            })?;
            merge_regular_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
        return Ok(());
    }

    if !source_metadata.is_file() {
        return Err(InstallationError::InvalidLegacyRoot {
            root: source.to_path_buf(),
            reason: "a Vapor-owned source contains a special filesystem entry",
        });
    }
    match fs::symlink_metadata(target) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(InstallationError::MigrationConflict {
                path: target.to_path_buf(),
            })
        }
        Ok(_) if !files_equal(source, target)? => Err(InstallationError::MigrationConflict {
            path: target.to_path_buf(),
        }),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|source| InstallationError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            fs::copy(source, target).map_err(|source_error| InstallationError::Io {
                path: source.to_path_buf(),
                source: source_error,
            })?;
            if !files_equal(source, target)? {
                return Err(InstallationError::MigrationCopyMismatch {
                    source: source.to_path_buf(),
                    destination: target.to_path_buf(),
                });
            }
            Ok(())
        }
        Err(source_error) => Err(InstallationError::Io {
            path: target.to_path_buf(),
            source: source_error,
        }),
    }
}

fn validate_retained_product_entry(
    product_root: &Path,
    relative_path: &Path,
) -> Result<(std::ffi::OsString, bool), InstallationError> {
    let mut components = relative_path.components();
    let name = match (components.next(), components.next()) {
        (Some(std::path::Component::Normal(name)), None) => name.to_os_string(),
        _ => {
            return Err(InstallationError::InvalidLegacyRoot {
                root: product_root.to_path_buf(),
                reason: "retained depot product and locator paths must be single top-level entries",
            });
        }
    };
    if name == std::ffi::OsStr::new("App.vapor.toml")
        || LEGACY_HOME_PATHS
            .iter()
            .any(|(legacy, _)| name == std::ffi::OsStr::new(legacy))
    {
        return Err(InstallationError::InvalidLegacyRoot {
            root: product_root.to_path_buf(),
            reason: "a retained Steam product entry overlaps Vapor-owned migration data",
        });
    }
    let entry = product_root.join(&name);
    let metadata = fs::symlink_metadata(&entry).map_err(|source| InstallationError::Io {
        path: entry.clone(),
        source,
    })?;
    if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
        return Err(InstallationError::InvalidLegacyRoot {
            root: product_root.to_path_buf(),
            reason: "a retained Steam product entry is a symlink or unsupported filesystem entry",
        });
    }
    Ok((name, metadata.is_dir()))
}

fn merge_legacy_bin(
    source: &Path,
    target: &Path,
    _relative: &Path,
) -> Result<(), InstallationError> {
    merge_regular_tree(source, target)
}

fn merge_legacy_vapor(source: &Path, staging: &Path) -> Result<Vec<PathBuf>, InstallationError> {
    let metadata = fs::symlink_metadata(source).map_err(|source_error| InstallationError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(InstallationError::InvalidLegacyRoot {
            root: source.to_path_buf(),
            reason: "the legacy .vapor entry must be an ordinary directory",
        });
    }

    let mut destinations = Vec::new();
    for entry in fs::read_dir(source).map_err(|source_error| InstallationError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })? {
        let entry = entry.map_err(|source_error| InstallationError::Io {
            path: source.to_path_buf(),
            source: source_error,
        })?;
        let name = entry.file_name();
        let destination = match name.to_str() {
            Some("diagnostics") => PathBuf::from("data/diagnostics"),
            Some("downloads") => PathBuf::from("cache/downloads"),
            Some("extract") => PathBuf::from("cache/extract"),
            Some("logs") => PathBuf::from("data/logs"),
            Some("state") => PathBuf::from("data/state"),
            _ => PathBuf::from("data/legacy-vapor").join(&name),
        };
        merge_regular_tree(&entry.path(), &staging.join(&destination))?;
        destinations.push(destination);
    }
    destinations.sort();
    destinations.dedup();
    Ok(destinations)
}

fn detach_legacy_bin(source: &Path, detached: &Path, _relative: &Path) -> io::Result<bool> {
    if let Some(parent) = detached.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(source, detached)?;
    Ok(true)
}

fn restore_migrated_bin(detached: &Path, product_bin: &Path) -> io::Result<()> {
    match fs::symlink_metadata(detached) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "detached Steam bin recovery path is not an ordinary directory",
            ));
        }
        Ok(_) => {}
    }
    fs::create_dir_all(product_bin)?;
    for entry in fs::read_dir(detached)? {
        let entry = entry?;
        let destination = product_bin.join(entry.file_name());
        match fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                restore_migrated_bin(&entry.path(), &destination)?;
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "recovery destination already exists: {}",
                        destination.display()
                    ),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::rename(entry.path(), destination)?;
            }
            Err(error) => return Err(error),
        }
    }
    fs::remove_dir(detached)
}

fn packagepack_executable_name() -> &'static str {
    if cfg!(windows) {
        "loo-cast-packagepack.exe"
    } else {
        "loo-cast-packagepack"
    }
}

fn validate_packagepack_depot(
    manifest_path: &Path,
    product_root: &Path,
) -> Result<Vec<PathBuf>, InstallationError> {
    let metadata = fs::symlink_metadata(manifest_path).map_err(|source| InstallationError::Io {
        path: manifest_path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(InstallationError::InvalidLegacyRoot {
            root: product_root.to_path_buf(),
            reason: "Packagepack.vapor.toml must be a regular file",
        });
    }
    let source = fs::read_to_string(manifest_path).map_err(|source| InstallationError::Io {
        path: manifest_path.to_path_buf(),
        source,
    })?;
    let package: toml::Value =
        toml::from_str(&source).map_err(|_| InstallationError::InvalidLegacyRoot {
            root: product_root.to_path_buf(),
            reason: "Packagepack.vapor.toml is invalid",
        })?;
    let section =
        package
            .get("packagepack")
            .ok_or_else(|| InstallationError::InvalidLegacyRoot {
                root: product_root.to_path_buf(),
                reason: "Packagepack.vapor.toml has no [packagepack] section",
            })?;
    let version_is_declared = section.get("version").is_some_and(|version| {
        version.as_str().is_some()
            || version.get("workspace").and_then(toml::Value::as_bool) == Some(true)
    });
    if section.get("name").and_then(toml::Value::as_str) != Some("loo-cast-packagepack")
        || !version_is_declared
        || !section
            .get("binaries")
            .and_then(toml::Value::as_array)
            .is_some_and(|names| {
                names
                    .iter()
                    .any(|name| name.as_str() == Some("loo-cast-packagepack"))
            })
        || package
            .get("packagepack")
            .and_then(|value| value.get("engine"))
            .and_then(|value| value.get("id"))
            .and_then(toml::Value::as_str)
            != Some("ghf-studios/loo-cast/spacetime-engine")
        || package
            .get("packagepack")
            .and_then(|value| value.get("game"))
            .and_then(|value| value.get("id"))
            .and_then(toml::Value::as_str)
            != Some("ghf-studios/loo-cast/loo-cast-game")
    {
        return Err(InstallationError::InvalidLegacyRoot {
            root: product_root.to_path_buf(),
            reason: "the manifest does not identify the approved Loo Cast Packagepack",
        });
    }
    packagepack_runtime_libraries(section, product_root)
}

fn packagepack_runtime_libraries(
    packagepack: &toml::Value,
    product_root: &Path,
) -> Result<Vec<PathBuf>, InstallationError> {
    let runtime = match packagepack.get("runtime") {
        Some(runtime) => {
            runtime
                .as_array()
                .ok_or_else(|| InstallationError::InvalidLegacyRoot {
                    root: product_root.to_path_buf(),
                    reason: "Packagepack runtime declarations must be an array",
                })?
        }
        None => return Ok(Vec::new()),
    };
    let target = crate::HostTarget::current()
        .map_err(|_| InstallationError::InvalidLegacyRoot {
            root: product_root.to_path_buf(),
            reason: "could not determine the host target for Packagepack runtime libraries",
        })?
        .triple()
        .to_owned();
    let mut libraries = Vec::new();
    for payload in runtime {
        let payload_target = payload.get("target").and_then(toml::Value::as_str);
        if payload_target != Some(target.as_str()) {
            continue;
        }
        let declared = match payload.get("libraries") {
            Some(libraries) => {
                libraries
                    .as_array()
                    .ok_or_else(|| InstallationError::InvalidLegacyRoot {
                        root: product_root.to_path_buf(),
                        reason: "Packagepack runtime libraries must be an array",
                    })?
            }
            None => continue,
        };
        for library in declared {
            let name = library
                .as_str()
                .ok_or_else(|| InstallationError::InvalidLegacyRoot {
                    root: product_root.to_path_buf(),
                    reason: "Packagepack runtime library names must be strings",
                })?;
            let mut components = Path::new(name).components();
            let valid_name = matches!(
                (components.next(), components.next()),
                (Some(std::path::Component::Normal(_)), None)
            );
            if !valid_name
                || name == "Packagepack.vapor.toml"
                || name == packagepack_executable_name()
            {
                return Err(InstallationError::InvalidLegacyRoot {
                    root: product_root.to_path_buf(),
                    reason: "Packagepack runtime libraries must be distinct top-level filenames",
                });
            }
            let relative = PathBuf::from(name);
            if libraries.contains(&relative) {
                return Err(InstallationError::InvalidLegacyRoot {
                    root: product_root.to_path_buf(),
                    reason: "a Packagepack runtime library is declared more than once",
                });
            }
            let path = product_root.join(&relative);
            let metadata = fs::symlink_metadata(&path).map_err(|source| InstallationError::Io {
                path: path.clone(),
                source,
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(InstallationError::InvalidLegacyRoot {
                    root: product_root.to_path_buf(),
                    reason:
                        "a declared Packagepack runtime library is missing or not a regular file",
                });
            }
            libraries.push(relative);
        }
    }
    libraries.sort();
    Ok(libraries)
}

fn validate_depot_product_manifest(
    app_manifest: &Path,
    product_root: &Path,
    retained_entries: &[(std::ffi::OsString, bool)],
) -> Result<(), InstallationError> {
    let source = fs::read_to_string(app_manifest).map_err(|source| InstallationError::Io {
        path: app_manifest.to_path_buf(),
        source,
    })?;
    let app: toml::Value = toml::from_str(&source).map_err(|_| {
        InstallationError::InvalidLegacyRoot {
            root: product_root.to_path_buf(),
            reason: "App.vapor.toml is invalid; refusing to migrate a root with an unknown product manifest",
        }
    })?;
    let declared = app
        .get("root")
        .and_then(|root| root.get("content"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| InstallationError::InvalidLegacyRoot {
            root: product_root.to_path_buf(),
            reason: "App.vapor.toml does not declare its depot products under [root.content]",
        })?;
    if declared.len() != 3 {
        return Err(InstallationError::InvalidLegacyRoot {
            root: product_root.to_path_buf(),
            reason:
                "App.vapor.toml must declare exactly the Engine, Game, and Packagepack products",
        });
    }

    let mut identities = std::collections::BTreeSet::new();
    let mut kinds = std::collections::BTreeSet::new();
    let mut product_paths = std::collections::BTreeSet::new();
    for item in declared {
        let id = item
            .get("id")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| InstallationError::InvalidLegacyRoot {
                root: product_root.to_path_buf(),
                reason: "a declared depot product is missing its content identity",
            })?;
        let kind = item
            .get("kind")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| InstallationError::InvalidLegacyRoot {
                root: product_root.to_path_buf(),
                reason: "a declared depot product is missing its content kind",
            })?;
        if item.get("workshop-id").is_some() {
            return Err(InstallationError::InvalidLegacyRoot {
                root: product_root.to_path_buf(),
                reason: "Loo Cast depot products must not refer to retired Workshop items",
            });
        }
        if item.get("source").and_then(toml::Value::as_str) != Some("steam-depot") {
            return Err(InstallationError::InvalidLegacyRoot {
                root: product_root.to_path_buf(),
                reason: "each retained product must declare source = \"steam-depot\"",
            });
        }
        let relative_path = item
            .get("path")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| InstallationError::InvalidLegacyRoot {
                root: product_root.to_path_buf(),
                reason: "each depot product must declare its relative Steam-root path",
            })?;
        let mut path_components = Path::new(relative_path).components();
        if !matches!(
            (path_components.next(), path_components.next()),
            (Some(std::path::Component::Normal(_)), None)
        ) {
            return Err(InstallationError::InvalidLegacyRoot {
                root: product_root.to_path_buf(),
                reason: "each depot product path must name one top-level Steam-root directory",
            });
        }
        if !product_paths.insert(relative_path.to_owned()) {
            return Err(InstallationError::InvalidLegacyRoot {
                root: product_root.to_path_buf(),
                reason: "App.vapor.toml repeats a depot product path",
            });
        }
        let parsed_kind = match kind {
            "engine" => crate::ContentKind::Engine,
            "game" => crate::ContentKind::Game,
            "packagepack" => crate::ContentKind::Packagepack,
            _ => {
                return Err(InstallationError::InvalidLegacyRoot {
                    root: product_root.to_path_buf(),
                    reason: "the three local products must be Engine, Game, and Packagepack",
                });
            }
        };
        let expected_id = match parsed_kind {
            crate::ContentKind::Engine => "ghf-studios/loo-cast/spacetime-engine",
            crate::ContentKind::Game => "ghf-studios/loo-cast/loo-cast-game",
            crate::ContentKind::Packagepack => "ghf-studios/loo-cast/loo-cast-packagepack",
            _ => {
                return Err(InstallationError::InvalidLegacyRoot {
                    root: product_root.to_path_buf(),
                    reason: "App.vapor.toml contains an unsupported depot product kind",
                });
            }
        };
        if id != expected_id {
            return Err(InstallationError::InvalidLegacyRoot {
                root: product_root.to_path_buf(),
                reason:
                    "App.vapor.toml does not declare the three approved Loo Cast product identities",
            });
        }
        if !identities.insert(id.to_owned()) || !kinds.insert(parsed_kind) {
            return Err(InstallationError::InvalidLegacyRoot {
                root: product_root.to_path_buf(),
                reason: "App.vapor.toml repeats a product identity or kind",
            });
        }
    }
    if kinds.len() != 3 {
        return Err(InstallationError::InvalidLegacyRoot {
            root: product_root.to_path_buf(),
            reason: "App.vapor.toml must contain one Engine, one Game, and one Packagepack",
        });
    }

    let mut mounted_identities = std::collections::BTreeSet::new();
    let mut mounted_kinds = std::collections::BTreeSet::new();
    let mut mounted_paths = std::collections::BTreeSet::new();
    for (name, is_directory) in retained_entries {
        if !is_directory {
            continue;
        }
        let root = product_root.join(name);
        let name = name
            .to_str()
            .ok_or_else(|| InstallationError::InvalidLegacyRoot {
                root: root.clone(),
                reason: "a retained product path cannot be represented in App.vapor.toml",
            })?;
        if !product_paths.contains(name) {
            return Err(InstallationError::InvalidLegacyRoot {
                root,
                reason: "retained product directory is not declared at its path by App.vapor.toml",
            });
        }
        let Some((kind, id)) = loo_product_identity(&root)? else {
            return Err(InstallationError::InvalidLegacyRoot {
                root,
                reason:
                    "a retained depot product directory has no recognized Vapor content manifest",
            });
        };
        if !identities.contains(&id) || !kinds.contains(&kind) {
            return Err(InstallationError::InvalidLegacyRoot {
                root,
                reason: "the retained depot product does not match App.vapor.toml",
            });
        }
        mounted_identities.insert(id);
        mounted_kinds.insert(kind);
        mounted_paths.insert(name.to_owned());
    }
    if mounted_identities != identities || mounted_kinds != kinds || mounted_paths != product_paths
    {
        return Err(InstallationError::InvalidLegacyRoot {
            root: product_root.to_path_buf(),
            reason: "retained depot product directories do not match all three declared product identities and paths",
        });
    }
    Ok(())
}

/// Accept only the exact pre-standalone Vapor Client manifest found in the
/// existing Loo Cast Steam install. This is migration evidence, not a valid
/// current content source: retired Workshop IDs are checked only to identify
/// the old install and are never used to acquire or publish content.
fn validate_legacy_vapor_client_manifest(
    app_manifest: &Path,
    product_root: &Path,
) -> Result<(), InstallationError> {
    let source = fs::read_to_string(app_manifest).map_err(|source| InstallationError::Io {
        path: app_manifest.to_path_buf(),
        source,
    })?;
    let invalid = || InstallationError::InvalidLegacyRoot {
        root: product_root.to_path_buf(),
        reason: "App.vapor.toml is not the known pre-standalone Vapor Client install manifest",
    };
    let document: toml::Value = toml::from_str(&source).map_err(|_| invalid())?;
    if document.get("schema").and_then(toml::Value::as_integer) != Some(1) {
        return Err(invalid());
    }
    let root = document.get("root").ok_or_else(invalid)?;
    if root.get("name").and_then(toml::Value::as_str) != Some("vapor-client")
        || root.get("organization").and_then(toml::Value::as_str) != Some("ghf-studios")
        || root.get("version").and_then(toml::Value::as_str) != Some("0.5.0")
        || root.get("repository").and_then(toml::Value::as_str)
            != Some("https://github.com/GHF-Studios/Vapor-Client")
        || root
            .get("steam")
            .and_then(|steam| steam.get("app-id"))
            .and_then(toml::Value::as_integer)
            != Some(2_122_620)
    {
        return Err(invalid());
    }

    let expected = [
        (
            "ghf-studios/loo-cast/spacetime-engine",
            "engine",
            "3762162251",
        ),
        ("ghf-studios/loo-cast/loo-cast-game", "game", "3762162407"),
        (
            "ghf-studios/loo-cast/loo-cast-packagepack",
            "packagepack",
            "3762162548",
        ),
    ];
    let products = root
        .get("content")
        .and_then(toml::Value::as_array)
        .ok_or_else(invalid)?;
    if products.len() != expected.len() {
        return Err(invalid());
    }
    for (product, (expected_id, expected_kind, expected_workshop_id)) in
        products.iter().zip(expected)
    {
        if product.get("id").and_then(toml::Value::as_str) != Some(expected_id)
            || product.get("kind").and_then(toml::Value::as_str) != Some(expected_kind)
            || product.get("app-id").and_then(toml::Value::as_integer) != Some(2_122_620)
            || product.get("workshop-id").and_then(toml::Value::as_str)
                != Some(expected_workshop_id)
            || (expected_kind == "packagepack"
                && product.get("default-launch").and_then(toml::Value::as_str) != Some("default"))
        {
            return Err(invalid());
        }
    }
    Ok(())
}

fn is_loo_cast_depot_product_root(path: &Path) -> Result<bool, InstallationError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(InstallationError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(false);
    }
    Ok(loo_product_identity(path)?.is_some())
}

fn loo_product_identity(
    path: &Path,
) -> Result<Option<(crate::ContentKind, String)>, InstallationError> {
    let products = [
        (
            crate::ContentKind::Engine,
            "spacetime-engine",
            "ghf-studios/loo-cast/spacetime-engine",
        ),
        (
            crate::ContentKind::Game,
            "loo-cast-game",
            "ghf-studios/loo-cast/loo-cast-game",
        ),
        (
            crate::ContentKind::Packagepack,
            "loo-cast-packagepack",
            "ghf-studios/loo-cast/loo-cast-packagepack",
        ),
    ];
    for (kind, expected_name, expected_id) in products {
        let manifest_path = path.join(kind.manifest_file_name());
        let metadata = match fs::symlink_metadata(&manifest_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(InstallationError::Io {
                    path: manifest_path,
                    source,
                });
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(InstallationError::InvalidLegacyRoot {
                root: path.to_path_buf(),
                reason: "a product manifest must be a regular file inside its product directory",
            });
        }
        let source =
            fs::read_to_string(&manifest_path).map_err(|source| InstallationError::Io {
                path: manifest_path.clone(),
                source,
            })?;
        let document: toml::Value =
            toml::from_str(&source).map_err(|_| InstallationError::InvalidLegacyRoot {
                root: path.to_path_buf(),
                reason: "a possible depot product has an invalid Vapor content manifest",
            })?;
        let name = document
            .get(kind.as_str())
            .and_then(|section| section.get("name"))
            .and_then(toml::Value::as_str)
            .ok_or_else(|| InstallationError::InvalidLegacyRoot {
                root: path.to_path_buf(),
                reason: "a product manifest is missing its kind-specific name",
            })?;
        if name != expected_name {
            continue;
        }
        return Ok(Some((kind, expected_id.to_owned())));
    }
    Ok(None)
}

fn files_equal(left: &Path, right: &Path) -> Result<bool, InstallationError> {
    let mut left_file = fs::File::open(left).map_err(|source| InstallationError::Io {
        path: left.to_path_buf(),
        source,
    })?;
    let mut right_file = fs::File::open(right).map_err(|source| InstallationError::Io {
        path: right.to_path_buf(),
        source,
    })?;
    let mut left_buffer = [0_u8; 16 * 1024];
    let mut right_buffer = [0_u8; 16 * 1024];
    loop {
        let left_read =
            left_file
                .read(&mut left_buffer)
                .map_err(|source| InstallationError::Io {
                    path: left.to_path_buf(),
                    source,
                })?;
        let right_read =
            right_file
                .read(&mut right_buffer)
                .map_err(|source| InstallationError::Io {
                    path: right.to_path_buf(),
                    source,
                })?;
        if left_read != right_read || left_buffer[..left_read] != right_buffer[..right_read] {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
    }
}

fn rollback_home_migration(
    home_root: &Path,
    staging: &Path,
    previous: &Path,
    promoted: &[PathBuf],
    backed_up: &[PathBuf],
) -> Vec<String> {
    let mut failures = Vec::new();
    for name in promoted.iter().rev() {
        let target = home_root.join(name);
        if let Err(error) = remove_path(&target) {
            failures.push(format!("remove {}: {error}", target.display()));
        }
    }
    for name in backed_up.iter().rev() {
        let backup = previous.join(name);
        let target = home_root.join(name);
        if let Err(error) = fs::rename(&backup, &target) {
            failures.push(format!("restore {}: {error}", target.display()));
        }
    }
    let _ = fs::remove_dir_all(staging);
    failures
}

fn remove_path(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::remove_dir_all(path)
        }
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn migration_nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

fn find_prefixed_entry(root: &Path, prefix: &str) -> Result<Option<PathBuf>, InstallationError> {
    for entry in fs::read_dir(root).map_err(|source| InstallationError::Io {
        path: root.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| InstallationError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        if entry.file_name().to_string_lossy().starts_with(prefix) {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

fn explicit_vapor_home() -> Option<PathBuf> {
    env::var_os(VAPOR_HOME_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn platform_home_root() -> Option<PathBuf> {
    #[cfg(windows)]
    let home = env::var_os("USERPROFILE").or_else(|| env::var_os("HOME"));
    #[cfg(not(windows))]
    let home = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE"));

    home.map(PathBuf::from).map(|home| home.join("Vapor"))
}

fn home_root_from_executable(executable: &Path) -> Option<PathBuf> {
    let executable = fs::canonicalize(executable).ok()?;
    let directory = executable.parent()?;
    if directory.file_name().is_some_and(|name| name == "bin") {
        directory.parent().map(Path::to_path_buf)
    } else if directory
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "bin")
    {
        directory.parent()?.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

#[derive(Debug)]
pub enum InstallationError {
    CurrentExecutable(io::Error),
    NotFound {
        executable: PathBuf,
    },
    HomeUnavailable,
    InvalidExplicitRoot {
        root: PathBuf,
        reason: &'static str,
    },
    InvalidBundle {
        root: PathBuf,
        reason: &'static str,
    },
    InvalidLegacyRoot {
        root: PathBuf,
        reason: &'static str,
    },
    MigrationConflict {
        path: PathBuf,
    },
    MigrationCopyMismatch {
        source: PathBuf,
        destination: PathBuf,
    },
    MigrationRecoveryRequired {
        path: PathBuf,
    },
    MigrationCleanupRequired {
        home: PathBuf,
        recovery: PathBuf,
        detail: String,
    },
    TransactionRollback {
        root: PathBuf,
        cause: String,
        rollback: String,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for InstallationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentExecutable(error) => write!(
                formatter,
                "failed to determine the running Vapor executable: {error}"
            ),
            Self::NotFound { executable } => write!(
                formatter,
                "no standalone Vapor home is active for `{}`; install Vapor or provide {VAPOR_HOME_ENV}",
                executable.display()
            ),
            Self::HomeUnavailable => {
                formatter.write_str("cannot determine the current user's home directory")
            }
            Self::InvalidExplicitRoot { root, reason } => write!(
                formatter,
                "invalid Vapor home `{}`: {reason}",
                root.display()
            ),
            Self::InvalidBundle { root, reason } => write!(
                formatter,
                "invalid standalone Vapor bundle `{}`: {reason}",
                root.display()
            ),
            Self::InvalidLegacyRoot { root, reason } => write!(
                formatter,
                "invalid legacy Loo Cast product root `{}`: {reason}",
                root.display()
            ),
            Self::MigrationConflict { path } => write!(
                formatter,
                "Vapor migration found different data already at `{}`; no source data was moved",
                path.display()
            ),
            Self::MigrationCopyMismatch {
                source,
                destination,
            } => write!(
                formatter,
                "Vapor migration could not verify `{}` after copying to `{}`; source data was left untouched",
                source.display(),
                destination.display()
            ),
            Self::MigrationRecoveryRequired { path } => write!(
                formatter,
                "a previous Vapor home migration needs recovery at `{}`",
                path.display()
            ),
            Self::MigrationCleanupRequired {
                home,
                recovery,
                detail,
            } => write!(
                formatter,
                "Vapor data is now installed at `{}`, but cleanup needs attention: {detail}; recoverable copy: `{}`",
                home.display(),
                recovery.display()
            ),
            Self::TransactionRollback {
                root,
                cause,
                rollback,
            } => write!(
                formatter,
                "Vapor filesystem transaction failed in `{}`: {cause}; rollback was incomplete: {rollback}",
                root.display()
            ),
            Self::Io { path, source } => {
                write!(formatter, "failed to access `{}`: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for InstallationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CurrentExecutable(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::NotFound { .. }
            | Self::HomeUnavailable
            | Self::InvalidExplicitRoot { .. }
            | Self::InvalidBundle { .. }
            | Self::InvalidLegacyRoot { .. }
            | Self::MigrationConflict { .. }
            | Self::MigrationCopyMismatch { .. }
            | Self::MigrationRecoveryRequired { .. }
            | Self::MigrationCleanupRequired { .. }
            | Self::TransactionRollback { .. } => None,
        }
    }
}

#[cfg(test)]
mod packagepack_depot_tests {
    use super::{packagepack_executable_name, VaporInstallation};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    fn fixture_root() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "vapor-packagepack-depot-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("create fixture root");
        path
    }

    fn package_manifest() -> &'static str {
        "schema = 1\n\n[packagepack]\nname = \"loo-cast-packagepack\"\nversion = \"0.5.0\"\nbinaries = [\"loo-cast-packagepack\"]\n\n[packagepack.engine]\nid = \"ghf-studios/loo-cast/spacetime-engine\"\n\n[packagepack.game]\nid = \"ghf-studios/loo-cast/loo-cast-game\"\n"
    }

    #[test]
    fn recognizes_the_installed_legacy_vapor_client_root_for_migration() {
        let root = fixture_root();
        let home = fixture_root();
        let app_manifest = r#"schema = 1

[root]
name = "vapor-client"
organization = "ghf-studios"
version = "0.5.0"
repository = "https://github.com/GHF-Studios/Vapor-Client"

[root.steam]
app-id = 2122620

[[root.content]]
id = "ghf-studios/loo-cast/spacetime-engine"
kind = "engine"
app-id = 2122620
workshop-id = "3762162251"

[[root.content]]
id = "ghf-studios/loo-cast/loo-cast-game"
kind = "game"
app-id = 2122620
workshop-id = "3762162407"

[[root.content]]
id = "ghf-studios/loo-cast/loo-cast-packagepack"
kind = "packagepack"
app-id = 2122620
workshop-id = "3762162548"
default-launch = "default"
"#;
        std::fs::write(root.join("App.vapor.toml"), app_manifest)
            .expect("write installed legacy manifest");
        std::fs::create_dir(root.join("bin")).expect("create legacy runtime directory");
        std::fs::create_dir(root.join("content")).expect("create legacy content directory");
        std::fs::write(root.join("bin/vapor.exe"), b"legacy Vapor runtime")
            .expect("write old runtime executable");
        std::fs::create_dir(root.join("content/installed"))
            .expect("create legacy installed content");
        std::fs::write(
            root.join("content/installed/receipt.json"),
            b"legacy receipt",
        )
        .expect("write legacy content receipt");

        let retained = VaporInstallation::legacy_product_entries(&root)
            .expect("recognize installed Vapor client root");
        assert!(retained.is_empty(), "all old Steam-root files must migrate");
        VaporInstallation::migrate_legacy_product_root(&root, &home, &retained)
            .expect("move old Vapor install into canonical home");
        assert!(!root.join("App.vapor.toml").exists());
        assert!(!root.join("bin").exists());
        assert!(!root.join("content").exists());
        assert!(home.join("App.vapor.toml").is_file());
        assert_eq!(
            std::fs::read(home.join("bin/vapor.exe")).unwrap(),
            b"legacy Vapor runtime"
        );
        assert_eq!(
            std::fs::read(home.join("content/installed/receipt.json")).unwrap(),
            b"legacy receipt"
        );

        std::fs::remove_dir_all(root).expect("remove fixture");
        std::fs::remove_dir_all(home).expect("remove home fixture");
    }

    #[test]
    fn current_depot_keeps_its_manifest_and_host_executable() {
        let root = fixture_root();
        std::fs::write(root.join("Packagepack.vapor.toml"), package_manifest())
            .expect("write package manifest");
        std::fs::write(
            root.join(if cfg!(windows) {
                "loo-cast-packagepack.exe"
            } else {
                "loo-cast-packagepack"
            }),
            b"fixture executable",
        )
        .expect("write package executable");
        std::fs::create_dir(root.join("old-vapor-runtime")).expect("write legacy runtime");

        let keep = VaporInstallation::legacy_product_entries(&root).expect("validate depot");
        assert_eq!(keep.len(), 2);
        assert!(keep.iter().any(|path| path == "Packagepack.vapor.toml"));
        assert!(keep.iter().any(|path| {
            path.file_name().and_then(|name| name.to_str())
                == Some(if cfg!(windows) {
                    "loo-cast-packagepack.exe"
                } else {
                    "loo-cast-packagepack"
                })
        }));

        let move_entries =
            VaporInstallation::legacy_entries_to_move(&root, &keep).expect("list legacy entries");
        assert_eq!(move_entries, vec![PathBuf::from("old-vapor-runtime")]);
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn current_depot_keeps_only_manifest_executable_and_declared_runtime_libraries() {
        let root = fixture_root();
        let target = crate::HostTarget::current()
            .expect("resolve host target")
            .triple();
        let library = if cfg!(windows) {
            "libunwind.dll"
        } else if cfg!(target_os = "macos") {
            "libvapor_runtime.dylib"
        } else {
            "libvapor_runtime.so"
        };
        let manifest = format!(
            "schema = 1\n\n[packagepack]\nname = \"loo-cast-packagepack\"\nversion = \"0.5.0\"\nbinaries = [\"loo-cast-packagepack\"]\nruntime = [{{ target = \"{target}\", libraries = [\"{library}\"] }}]\n\n[packagepack.engine]\nid = \"ghf-studios/loo-cast/spacetime-engine\"\n\n[packagepack.game]\nid = \"ghf-studios/loo-cast/loo-cast-game\"\n"
        );
        std::fs::write(root.join("Packagepack.vapor.toml"), manifest)
            .expect("write package manifest");
        std::fs::write(
            root.join(packagepack_executable_name()),
            b"package executable",
        )
        .expect("write package executable");
        std::fs::write(root.join(library), b"declared runtime library")
            .expect("write runtime library");
        std::fs::write(root.join("unlisted.dll"), b"undeclared file")
            .expect("write unrelated file");

        let keep = VaporInstallation::legacy_product_entries(&root)
            .expect("validate Packagepack depot with declared sidecar");
        assert_eq!(keep.len(), 3);
        assert!(keep.contains(&PathBuf::from("Packagepack.vapor.toml")));
        assert!(keep.contains(&PathBuf::from(packagepack_executable_name())));
        assert!(keep.contains(&PathBuf::from(library)));
        assert!(!keep.contains(&PathBuf::from("unlisted.dll")));

        let move_entries = VaporInstallation::legacy_entries_to_move(&root, &keep)
            .expect("list files that must leave the Steam root");
        assert_eq!(move_entries, vec![PathBuf::from("unlisted.dll")]);
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn current_depot_rejects_a_packagepack_manifest_for_another_product() {
        let root = fixture_root();
        let manifest = package_manifest().replace("loo-cast-packagepack", "other-product");
        std::fs::write(root.join("Packagepack.vapor.toml"), manifest)
            .expect("write invalid package manifest");

        assert!(VaporInstallation::legacy_product_entries(&root).is_err());
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn current_depot_accepts_the_workspace_version_form_used_by_source_manifests() {
        let root = fixture_root();
        let manifest =
            package_manifest().replace("version = \"0.5.0\"", "version.workspace = true");
        std::fs::write(root.join("Packagepack.vapor.toml"), manifest)
            .expect("write source-style package manifest");
        std::fs::write(
            root.join(packagepack_executable_name()),
            b"package executable",
        )
        .expect("write package executable");

        let keep = VaporInstallation::legacy_product_entries(&root)
            .expect("validate source-style Packagepack manifest");
        assert_eq!(keep.len(), 2);
        assert!(keep.contains(&PathBuf::from("Packagepack.vapor.toml")));
        assert!(keep.contains(&PathBuf::from(packagepack_executable_name())));
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn old_three_product_depot_is_migration_input_not_steam_payload() {
        let root = fixture_root();
        let products = [
            ("spacetime-engine", "Engine", "engine", "spacetime-engine"),
            ("loo-cast-game", "Game", "game", "loo-cast-game"),
            (
                "loo-cast-packagepack",
                "Packagepack",
                "packagepack",
                "loo-cast-packagepack",
            ),
        ];
        for (directory, filename, section, name) in products {
            let product = root.join(directory);
            std::fs::create_dir_all(&product).expect("create legacy product root");
            std::fs::write(
                product.join(format!("{filename}.vapor.toml")),
                format!("schema = 1\n\n[{section}]\nname = \"{name}\"\n"),
            )
            .expect("write legacy product manifest");
        }
        let app_manifest = r#"schema = 1

[[root.content]]
id = "ghf-studios/loo-cast/spacetime-engine"
kind = "engine"
app-id = 2122620
source = "steam-depot"
path = "spacetime-engine"

[[root.content]]
id = "ghf-studios/loo-cast/loo-cast-game"
kind = "game"
app-id = 2122620
source = "steam-depot"
path = "loo-cast-game"

[[root.content]]
id = "ghf-studios/loo-cast/loo-cast-packagepack"
kind = "packagepack"
app-id = 2122620
source = "steam-depot"
path = "loo-cast-packagepack"
"#;
        std::fs::write(root.join("App.vapor.toml"), app_manifest)
            .expect("write legacy app manifest");

        let keep = VaporInstallation::legacy_product_entries(&root)
            .expect("validate recognized legacy install");
        assert!(
            keep.is_empty(),
            "old loose products must not remain in Steam"
        );
        let move_entries =
            VaporInstallation::legacy_entries_to_move(&root, &keep).expect("list legacy entries");
        assert!(move_entries.contains(&PathBuf::from("App.vapor.toml")));
        for (directory, _, _, _) in products {
            assert!(move_entries.contains(&PathBuf::from(directory)));
        }
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn migration_moves_old_runtime_and_manifest_but_retains_declared_packagepack_files() {
        let fixture = fixture_root();
        let product_root = fixture.join("Loo Cast");
        let home = fixture.join("Vapor");
        std::fs::create_dir_all(&product_root).expect("create product root");
        let target = crate::HostTarget::current()
            .expect("resolve host target")
            .triple();
        let library = if cfg!(windows) {
            "libunwind.dll"
        } else if cfg!(target_os = "macos") {
            "libvapor_runtime.dylib"
        } else {
            "libvapor_runtime.so"
        };
        let manifest = package_manifest().replace(
            "binaries = [\"loo-cast-packagepack\"]",
            &format!(
                "binaries = [\"loo-cast-packagepack\"]\nruntime = [{{ target = \"{target}\", libraries = [\"{library}\"] }}]"
            ),
        );
        std::fs::write(product_root.join("Packagepack.vapor.toml"), manifest)
            .expect("write package manifest");
        let executable_name = if cfg!(windows) {
            "loo-cast-packagepack.exe"
        } else {
            "loo-cast-packagepack"
        };
        std::fs::write(product_root.join(executable_name), b"fixture executable")
            .expect("write package executable");
        std::fs::write(product_root.join(library), b"declared runtime library")
            .expect("write Packagepack runtime library");
        std::fs::write(product_root.join("App.vapor.toml"), b"old app manifest")
            .expect("write old app manifest");
        std::fs::create_dir_all(product_root.join("bin")).expect("create old bin");
        std::fs::write(product_root.join("bin/vapor.exe"), b"old runtime")
            .expect("write old runtime");
        std::fs::create_dir_all(product_root.join("output")).expect("create old output");
        std::fs::write(product_root.join("output/receipt"), b"old output")
            .expect("write old output marker");

        let retained = VaporInstallation::legacy_product_entries(&product_root)
            .expect("validate current Steam package");
        let report =
            VaporInstallation::migrate_legacy_product_root(&product_root, &home, &retained)
                .expect("migrate old Vapor files");

        assert_eq!(
            report.source_product_manifest,
            std::fs::canonicalize(&product_root)
                .unwrap()
                .join("Packagepack.vapor.toml")
        );
        assert_eq!(report.retained_product_entries.len(), 3);
        assert!(product_root.join("Packagepack.vapor.toml").is_file());
        assert!(product_root.join(executable_name).is_file());
        assert!(product_root.join(library).is_file());
        assert!(!product_root.join("App.vapor.toml").exists());
        assert!(!product_root.join("bin").exists());
        assert!(!product_root.join("output").exists());
        assert!(home.join("App.vapor.toml").is_file());
        assert_eq!(
            std::fs::read(home.join("bin/vapor.exe")).unwrap(),
            b"old runtime"
        );
        assert_eq!(
            std::fs::read(home.join("output/receipt")).unwrap(),
            b"old output"
        );

        std::fs::remove_dir_all(fixture).expect("remove fixture");
    }
}
