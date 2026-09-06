//! Reconciliation between Vapor semantic dependencies and Cargo dependencies.
//!
//! Vapor decides which semantic Content dependency must exist.
//! Cargo remains responsible for expressing and resolving the physical Rust
//! dependency graph.
//!
//! The first reconciliation slice is deliberately conservative:
//!
//! - verify direct dependencies of one Library;
//! - add missing Cargo dependencies using `cargo add`;
//! - never overwrite a conflicting existing Cargo dependency;
//! - verify the resulting physical edge using `cargo metadata`.

use crate::{
    CargoInspectionError, CargoPackageInspection, ContentKind, ContentVersionId, LocalCatalog,
    LocalContent, ManagedToolchain, ResolvedContentGraph, ToolchainError,
    inspect_local_cargo_package,
};
use pathdiff::diff_paths;
use semver::{Version, VersionReq};
use serde::Deserialize;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

/// Physical Cargo state for one Vapor dependency binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CargoDependencyState {
    /// Cargo declares and resolves this binding to the expected physical
    /// package.
    Valid,

    /// Vapor requires this binding, but Cargo does not currently declare it.
    Missing,

    /// Cargo declares the expected package/path, but physical resolution has
    /// not yet been checked because another binding conflicted.
    Declared,

    /// Cargo already owns this binding, but it describes some different
    /// physical dependency.
    Conflict { declarations: Vec<String> },

    /// The declaration looked correct, but Cargo's resolved graph did not
    /// contain the expected edge.
    Unresolved { message: String },
}

impl CargoDependencyState {
    pub fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }

    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }

    pub fn prevents_safe_repair(&self) -> bool {
        matches!(
            self,
            Self::Conflict { .. } | Self::Unresolved { .. } | Self::Declared
        )
    }
}

impl fmt::Display for CargoDependencyState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Valid => formatter.write_str("valid"),
            Self::Missing => formatter.write_str("missing"),
            Self::Declared => formatter.write_str("declared"),
            Self::Conflict { .. } => formatter.write_str("conflict"),
            Self::Unresolved { .. } => formatter.write_str("unresolved"),
        }
    }
}

/// Reconciliation state for one direct Vapor dependency of a Library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoDependencyReconciliation {
    /// Vapor-local dependency binding.
    ///
    /// For this first Rust-backed slice the binding is also the intended Cargo
    /// dependency alias.
    pub binding: String,

    /// Exact resolved Vapor identity.
    pub dependency: ContentVersionId,

    /// Physical Cargo package expected to realize that dependency.
    pub package: CargoPackageInspection,

    pub state: CargoDependencyState,
}

/// Cargo realization state for one resolved Vapor Library root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryCargoReconciliation {
    pub library: ContentVersionId,
    pub package: CargoPackageInspection,
    pub dependencies: Vec<CargoDependencyReconciliation>,
}

impl LibraryCargoReconciliation {
    pub fn is_valid(&self) -> bool {
        self.dependencies
            .iter()
            .all(|dependency| dependency.state.is_valid())
    }

    pub fn missing_bindings(&self) -> impl Iterator<Item = &str> {
        self.dependencies
            .iter()
            .filter(|dependency| dependency.state.is_missing())
            .map(|dependency| dependency.binding.as_str())
    }
}

/// Result of an explicit Cargo repair operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoRepairReport {
    pub added_bindings: Vec<String>,
    pub reconciliation: LibraryCargoReconciliation,
}

/// Verify that the resolved direct Vapor dependencies of one Library are
/// physically represented by Cargo.
///
/// The current slice intentionally does not reconcile arbitrary behavioral
/// Content yet. Library dependency realization is the architecture-proving
/// case from which broader Rust-backed Content semantics can be learned.
pub fn verify_local_library_cargo_dependencies(
    catalog: &LocalCatalog,
    graph: &ResolvedContentGraph,
) -> Result<LibraryCargoReconciliation, CargoReconciliationError> {
    if graph.root_node().kind != ContentKind::Library {
        return Err(CargoReconciliationError::RootNotLibrary {
            identity: graph.root.clone(),
            actual_kind: graph.root_node().kind,
        });
    }

    let library_content = local(catalog, &graph.root)?;
    let library_package = inspect_local_cargo_package(library_content)?;

    require_library_target(&graph.root, &library_package)?;

    let declared_metadata = load_cargo_metadata(library_content, MetadataScope::DeclaredOnly)?;

    let declared_library_package =
        metadata_package_for_manifest(&declared_metadata, &library_package.manifest_path)?;

    let mut dependencies = Vec::new();

    for (binding, dependency_identity) in &graph.root_node().dependencies {
        let dependency_content = local(catalog, dependency_identity)?;
        let dependency_package = inspect_local_cargo_package(dependency_content)?;

        require_library_target(dependency_identity, &dependency_package)?;

        let matching_bindings: Vec<_> = declared_library_package
            .dependencies
            .iter()
            .filter(|dependency| dependency.is_normal_unconditional())
            .filter(|dependency| dependency.binding() == binding)
            .collect();

        let state = match matching_bindings.as_slice() {
            [] => CargoDependencyState::Missing,

            [declaration] if declaration_matches_package(declaration, &dependency_package) => {
                CargoDependencyState::Declared
            }

            declarations => CargoDependencyState::Conflict {
                declarations: declarations
                    .iter()
                    .map(|declaration| declaration.describe())
                    .collect(),
            },
        };

        dependencies.push(CargoDependencyReconciliation {
            binding: binding.clone(),
            dependency: dependency_identity.clone(),
            package: dependency_package,
            state,
        });
    }

    let has_conflict = dependencies
        .iter()
        .any(|dependency| matches!(dependency.state, CargoDependencyState::Conflict { .. }));

    if !has_conflict
        && dependencies
            .iter()
            .any(|dependency| matches!(dependency.state, CargoDependencyState::Declared))
    {
        let resolved_metadata = load_cargo_metadata(library_content, MetadataScope::ResolvedGraph)?;

        for dependency in &mut dependencies {
            if !matches!(dependency.state, CargoDependencyState::Declared) {
                continue;
            }

            dependency.state = if resolved_edge_matches(
                &resolved_metadata,
                &library_package.manifest_path,
                &dependency.binding,
                &dependency.package.manifest_path,
            )? {
                CargoDependencyState::Valid
            } else {
                CargoDependencyState::Unresolved {
                    message: format!(
                        "Cargo did not resolve `{}` to package `{}` at `{}`",
                        dependency.binding,
                        dependency.package.name,
                        dependency.package.manifest_path.display()
                    ),
                }
            };
        }
    }

    Ok(LibraryCargoReconciliation {
        library: graph.root.clone(),
        package: library_package,
        dependencies,
    })
}

/// Repair missing Cargo dependencies required by one Vapor Library.
///
/// This operation deliberately refuses to overwrite conflicts.
///
/// Without stored previous-realization state, Vapor cannot yet distinguish an
/// automatically stale Cargo entry from an intentional developer edit. Missing
/// bindings are safe to add; conflicting bindings require future reconciliation
/// history/override semantics.
pub fn repair_local_library_cargo_dependencies(
    catalog: &LocalCatalog,
    graph: &ResolvedContentGraph,
) -> Result<CargoRepairReport, CargoReconciliationError> {
    let before = verify_local_library_cargo_dependencies(catalog, graph)?;

    let unsafe_bindings: Vec<_> = before
        .dependencies
        .iter()
        .filter(|dependency| dependency.state.prevents_safe_repair())
        .map(|dependency| dependency.binding.clone())
        .collect();

    if !unsafe_bindings.is_empty() {
        return Err(CargoReconciliationError::UnsafeRepair {
            identity: before.library,
            bindings: unsafe_bindings,
        });
    }

    let library_content = local(catalog, &graph.root)?;

    let missing: Vec<_> = before
        .dependencies
        .iter()
        .filter(|dependency| dependency.state.is_missing())
        .cloned()
        .collect();

    let mut added_bindings = Vec::new();

    for dependency in missing {
        let dependency_content = local(catalog, &dependency.dependency)?;

        cargo_add_dependency(
            library_content,
            &before.package,
            &dependency.binding,
            dependency_content,
            &dependency.package,
        )?;

        added_bindings.push(dependency.binding);
    }

    let reconciliation = verify_local_library_cargo_dependencies(catalog, graph)?;

    if !reconciliation.is_valid() {
        return Err(CargoReconciliationError::RepairIncomplete {
            identity: reconciliation.library.clone(),
            added_bindings,
        });
    }

    Ok(CargoRepairReport {
        added_bindings,
        reconciliation,
    })
}

fn cargo_add_dependency(
    depender_content: &LocalContent,
    depender_package: &CargoPackageInspection,
    binding: &str,
    dependency_content: &LocalContent,
    dependency_package: &CargoPackageInspection,
) -> Result<(), CargoReconciliationError> {
    let depender_root = fs::canonicalize(&depender_content.root).map_err(|source| {
        CargoReconciliationError::Io {
            path: depender_content.root.clone(),
            source,
        }
    })?;

    let dependency_root = fs::canonicalize(&dependency_content.root).map_err(|source| {
        CargoReconciliationError::Io {
            path: dependency_content.root.clone(),
            source,
        }
    })?;

    let relative_path = diff_paths(&dependency_root, &depender_root).ok_or_else(|| {
        CargoReconciliationError::CannotRelativizeDependency {
            depender: depender_root.clone(),
            dependency: dependency_root.clone(),
        }
    })?;

    let toolchain = ManagedToolchain::discover()?;
    let mut command = toolchain.cargo_command()?;

    command
        .arg("add")
        .arg("--quiet")
        .arg("--path")
        .arg(&relative_path)
        .arg("--manifest-path")
        .arg(&depender_package.manifest_path)
        .arg("--package")
        .arg(&depender_package.name)
        .current_dir(&depender_root);

    if binding != dependency_package.name {
        command.arg("--rename").arg(binding);
    }

    let output = command
        .output()
        .map_err(|source| CargoReconciliationError::Io {
            path: depender_package.manifest_path.clone(),
            source,
        })?;

    if output.status.success() {
        Ok(())
    } else {
        Err(CargoReconciliationError::CargoAddFailed {
            identity: depender_content.version_id(),
            binding: binding.to_owned(),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }
}

fn require_library_target(
    identity: &ContentVersionId,
    package: &CargoPackageInspection,
) -> Result<(), CargoReconciliationError> {
    if package.has_library_target() {
        Ok(())
    } else {
        Err(CargoReconciliationError::PackageHasNoLibraryTarget {
            identity: identity.clone(),
            package: package.name.clone(),
            manifest_path: package.manifest_path.clone(),
        })
    }
}

fn declaration_matches_package(
    declaration: &CargoMetadataDependency,
    package: &CargoPackageInspection,
) -> bool {
    if declaration.name != package.name {
        return false;
    }

    if !declaration.requirement.matches(&package.version) {
        return false;
    }

    let Some(actual_path) = declaration.path.as_deref() else {
        return false;
    };

    let Some(expected_root) = package.manifest_path.parent() else {
        return false;
    };

    paths_match(actual_path, expected_root)
}

fn resolved_edge_matches(
    metadata: &CargoMetadataDocument,
    depender_manifest: &Path,
    binding: &str,
    dependency_manifest: &Path,
) -> Result<bool, CargoReconciliationError> {
    let depender = metadata_package_for_manifest(metadata, depender_manifest)?;

    let Some(dependency) = metadata
        .packages
        .iter()
        .find(|package| paths_match(&package.manifest_path, dependency_manifest))
    else {
        return Ok(false);
    };

    let resolve = metadata
        .resolve
        .as_ref()
        .ok_or(CargoReconciliationError::MissingResolvedCargoGraph)?;

    let depender_node = resolve
        .nodes
        .iter()
        .find(|node| node.id == depender.id)
        .ok_or_else(|| CargoReconciliationError::MissingResolvedCargoNode {
            package_id: depender.id.clone(),
        })?;

    Ok(depender_node.dependencies.iter().any(|resolved| {
        resolved.pkg == dependency.id && resolved_binding_matches(&resolved.name, binding)
    }))
}

fn resolved_binding_matches(actual: &str, expected: &str) -> bool {
    actual == expected || actual == expected.replace('-', "_")
}

fn local<'a>(
    catalog: &'a LocalCatalog,
    identity: &ContentVersionId,
) -> Result<&'a LocalContent, CargoReconciliationError> {
    catalog
        .get(identity)
        .ok_or_else(|| CargoReconciliationError::MissingLocalContent {
            identity: identity.clone(),
        })
}

#[derive(Debug, Clone, Copy)]
enum MetadataScope {
    DeclaredOnly,
    ResolvedGraph,
}

fn load_cargo_metadata(
    content: &LocalContent,
    scope: MetadataScope,
) -> Result<CargoMetadataDocument, CargoReconciliationError> {
    let manifest_path = content.root.join("Cargo.toml");

    let manifest_path =
        fs::canonicalize(&manifest_path).map_err(|source| CargoReconciliationError::Io {
            path: manifest_path.clone(),
            source,
        })?;

    let toolchain = ManagedToolchain::discover()?;
    let mut command = toolchain.cargo_command()?;

    command
        .arg("metadata")
        .args(["--format-version", "1"])
        .arg("--manifest-path")
        .arg(&manifest_path)
        .current_dir(&content.root);

    if matches!(scope, MetadataScope::DeclaredOnly) {
        command.arg("--no-deps");
    }

    let output = command
        .output()
        .map_err(|source| CargoReconciliationError::Io {
            path: manifest_path.clone(),
            source,
        })?;

    if !output.status.success() {
        return Err(CargoReconciliationError::CargoMetadataFailed {
            manifest_path,
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }

    serde_json::from_slice(&output.stdout).map_err(|source| {
        CargoReconciliationError::InvalidCargoMetadata {
            manifest_path,
            source,
        }
    })
}

fn metadata_package_for_manifest<'a>(
    metadata: &'a CargoMetadataDocument,
    manifest_path: &Path,
) -> Result<&'a CargoMetadataPackage, CargoReconciliationError> {
    let matching: Vec<_> = metadata
        .packages
        .iter()
        .filter(|package| paths_match(&package.manifest_path, manifest_path))
        .collect();

    match matching.as_slice() {
        [package] => Ok(*package),

        _ => Err(CargoReconciliationError::AmbiguousMetadataPackage {
            manifest_path: manifest_path.to_path_buf(),
            packages: matching
                .iter()
                .map(|package| {
                    format!(
                        "{} {} ({})",
                        package.name,
                        package.version,
                        package.manifest_path.display()
                    )
                })
                .collect(),
        }),
    }
}

fn paths_match(left: &Path, right: &Path) -> bool {
    let left = fs::canonicalize(left).unwrap_or_else(|_| left.to_path_buf());
    let right = fs::canonicalize(right).unwrap_or_else(|_| right.to_path_buf());

    left == right
}

#[derive(Debug, Deserialize)]
struct CargoMetadataDocument {
    packages: Vec<CargoMetadataPackage>,
    resolve: Option<CargoMetadataResolve>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataPackage {
    id: String,
    name: String,
    version: Version,
    manifest_path: PathBuf,

    #[serde(default)]
    dependencies: Vec<CargoMetadataDependency>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataDependency {
    name: String,

    #[serde(rename = "req")]
    requirement: VersionReq,

    kind: Option<String>,
    rename: Option<String>,
    path: Option<PathBuf>,
    target: Option<String>,
}

impl CargoMetadataDependency {
    fn binding(&self) -> &str {
        self.rename.as_deref().unwrap_or(&self.name)
    }

    fn is_normal_unconditional(&self) -> bool {
        self.kind.is_none() && self.target.is_none()
    }

    fn describe(&self) -> String {
        let source = match &self.path {
            Some(path) => path.display().to_string(),
            None => "non-path source".to_owned(),
        };

        format!(
            "{} -> {} {} ({source})",
            self.binding(),
            self.name,
            self.requirement
        )
    }
}

#[derive(Debug, Deserialize)]
struct CargoMetadataResolve {
    nodes: Vec<CargoMetadataResolveNode>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataResolveNode {
    id: String,

    #[serde(default, rename = "deps")]
    dependencies: Vec<CargoMetadataResolvedDependency>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataResolvedDependency {
    name: String,
    pkg: String,
}

#[derive(Debug)]
pub enum CargoReconciliationError {
    RootNotLibrary {
        identity: ContentVersionId,
        actual_kind: ContentKind,
    },

    MissingLocalContent {
        identity: ContentVersionId,
    },

    PackageHasNoLibraryTarget {
        identity: ContentVersionId,
        package: String,
        manifest_path: PathBuf,
    },

    CannotRelativizeDependency {
        depender: PathBuf,
        dependency: PathBuf,
    },

    UnsafeRepair {
        identity: ContentVersionId,
        bindings: Vec<String>,
    },

    RepairIncomplete {
        identity: ContentVersionId,
        added_bindings: Vec<String>,
    },

    AmbiguousMetadataPackage {
        manifest_path: PathBuf,
        packages: Vec<String>,
    },

    MissingResolvedCargoGraph,

    MissingResolvedCargoNode {
        package_id: String,
    },

    InvalidCargoMetadata {
        manifest_path: PathBuf,
        source: serde_json::Error,
    },

    CargoMetadataFailed {
        manifest_path: PathBuf,
        status: ExitStatus,
        stderr: String,
    },

    CargoAddFailed {
        identity: ContentVersionId,
        binding: String,
        status: ExitStatus,
        stderr: String,
    },

    Inspection(CargoInspectionError),

    Toolchain(ToolchainError),

    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl From<CargoInspectionError> for CargoReconciliationError {
    fn from(error: CargoInspectionError) -> Self {
        Self::Inspection(error)
    }
}

impl From<ToolchainError> for CargoReconciliationError {
    fn from(error: ToolchainError) -> Self {
        Self::Toolchain(error)
    }
}

impl fmt::Display for CargoReconciliationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RootNotLibrary {
                identity,
                actual_kind,
            } => {
                write!(
                    formatter,
                    "Cargo Library reconciliation requires a Library root; \
                     `{identity}` is `{actual_kind}`"
                )
            }

            Self::MissingLocalContent { identity } => {
                write!(
                    formatter,
                    "resolved Vapor Content `{identity}` is not locally available"
                )
            }

            Self::PackageHasNoLibraryTarget {
                identity,
                package,
                manifest_path,
            } => {
                write!(
                    formatter,
                    "Vapor Content `{identity}` maps to Cargo package `{package}` at `{}`, \
                     but that package exposes no Rust library target",
                    manifest_path.display()
                )
            }

            Self::CannotRelativizeDependency {
                depender,
                dependency,
            } => {
                write!(
                    formatter,
                    "cannot construct a relative Cargo path from `{}` to `{}`",
                    depender.display(),
                    dependency.display()
                )
            }

            Self::UnsafeRepair { identity, bindings } => {
                write!(
                    formatter,
                    "refusing to repair Cargo dependencies for `{identity}` because \
                     existing state conflicts on: {}",
                    bindings.join(", ")
                )
            }

            Self::RepairIncomplete {
                identity,
                added_bindings,
            } => {
                write!(
                    formatter,
                    "Cargo repair for `{identity}` added [{}], but verification \
                     still does not match the resolved Vapor graph",
                    added_bindings.join(", ")
                )
            }

            Self::AmbiguousMetadataPackage {
                manifest_path,
                packages,
            } => {
                write!(
                    formatter,
                    "could not uniquely identify Cargo package for `{}`",
                    manifest_path.display()
                )?;

                if !packages.is_empty() {
                    write!(formatter, ": {}", packages.join(", "))?;
                }

                Ok(())
            }

            Self::MissingResolvedCargoGraph => {
                formatter.write_str("Cargo metadata did not contain a resolved dependency graph")
            }

            Self::MissingResolvedCargoNode { package_id } => {
                write!(
                    formatter,
                    "Cargo metadata did not contain resolved node `{package_id}`"
                )
            }

            Self::InvalidCargoMetadata {
                manifest_path,
                source,
            } => {
                write!(
                    formatter,
                    "Cargo returned invalid metadata for `{}`: {source}",
                    manifest_path.display()
                )
            }

            Self::CargoMetadataFailed {
                manifest_path,
                status,
                stderr,
            } => {
                write!(
                    formatter,
                    "Cargo metadata failed for `{}` with {status}",
                    manifest_path.display()
                )?;

                if !stderr.is_empty() {
                    write!(formatter, ": {stderr}")?;
                }

                Ok(())
            }

            Self::CargoAddFailed {
                identity,
                binding,
                status,
                stderr,
            } => {
                write!(
                    formatter,
                    "Cargo failed to add Vapor dependency `{binding}` for \
                     `{identity}` with {status}"
                )?;

                if !stderr.is_empty() {
                    write!(formatter, ": {stderr}")?;
                }

                Ok(())
            }

            Self::Inspection(error) => write!(formatter, "{error}"),

            Self::Toolchain(error) => write!(formatter, "{error}"),

            Self::Io { path, source } => {
                write!(formatter, "failed to access `{}`: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for CargoReconciliationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidCargoMetadata { source, .. } => Some(source),
            Self::Inspection(error) => Some(error),
            Self::Toolchain(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
