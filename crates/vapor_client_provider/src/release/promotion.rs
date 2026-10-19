//! Promotion of built artifacts into the Vapor installation.

use crate::{
    cross_toolchain,
    discovery::{EnvironmentPaths, ensure_contained},
    target::host_runtime_target,
    workspace::WorkspaceManifest,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// Promote configured artifacts for the host target.
pub fn promote(paths: &EnvironmentPaths, manifest: &WorkspaceManifest) -> Result<usize, String> {
    promote_for_targets(paths, manifest, &[])
}

/// Promote configured artifacts for one or more runtime targets.
///
/// An empty target list uses Cargo's host-default output layout. Explicit
/// targets use the managed target-specific output layout.
pub fn promote_for_targets(
    paths: &EnvironmentPaths,
    manifest: &WorkspaceManifest,
    target_triples: &[String],
) -> Result<usize, String> {
    promote_for_targets_in_profile(paths, manifest, target_triples, "dev")
}

/// Promote configured release-profile artifacts for one or more targets.
pub fn promote_release_for_targets(
    paths: &EnvironmentPaths,
    manifest: &WorkspaceManifest,
    target_triples: &[String],
) -> Result<usize, String> {
    promote_for_targets_in_profile(paths, manifest, target_triples, "release")
}

fn promote_for_targets_in_profile(
    paths: &EnvironmentPaths,
    manifest: &WorkspaceManifest,
    target_triples: &[String],
    profile: &str,
) -> Result<usize, String> {
    let targets = RuntimeTarget::many(target_triples)?;
    let installation = paths.installation().root();
    let mut promoted = 0;
    for project in manifest.cargo_projects() {
        for binary in project.binaries() {
            for target in &targets {
                let filename = target.binary_file_name(binary);
                let source = target
                    .build_output_root(installation, project.name(), profile)
                    .join(&filename);
                ensure_contained(installation, &source)?;
                if !source.is_file() {
                    return Err(format!(
                        "built binary is missing for project '{}' target '{}': {}",
                        project.name(),
                        target.triple(),
                        source.display()
                    ));
                }
                let destination = installation
                    .join("bin")
                    .join(target.triple())
                    .join(&filename);
                ensure_contained(installation, &destination)?;
                promote_file(&source, &destination)?;
                let destination_root = destination.parent().ok_or_else(|| {
                    format!(
                        "promoted binary destination has no parent: {}",
                        destination.display()
                    )
                })?;
                cross_toolchain::copy_windows_runtime_dlls(
                    installation,
                    target.triple(),
                    destination_root,
                )?;
                promoted += 1;
            }
        }
        for target in &targets {
            for library in project.libraries(target.triple()) {
                let source = target
                    .build_output_root(installation, project.name(), profile)
                    .join(library);
                ensure_contained(installation, &source)?;
                if !source.is_file() {
                    return Err(format!(
                        "built runtime library is missing for project '{}' target '{}': {}",
                        project.name(),
                        target.triple(),
                        source.display()
                    ));
                }
                let destination = installation.join("bin").join(target.triple()).join(library);
                ensure_contained(installation, &destination)?;
                promote_file(&source, &destination)?;
                promoted += 1;
            }
        }
    }
    for target in &targets {
        let bin_root = installation.join("bin").join(target.triple());
        for legacy in [
            "vapor-devtools",
            "vapor-studio-service",
            "vapor-monitor",
            "vapor-profile",
        ] {
            let path = bin_root.join(target.binary_file_name(legacy));
            if path.is_file() {
                fs::remove_file(&path).map_err(|error| {
                    format!(
                        "failed to remove retired binary '{}': {error}",
                        path.display()
                    )
                })?;
            }
        }
    }
    Ok(promoted)
}

#[derive(Debug, Clone)]
struct RuntimeTarget {
    triple: String,
    explicit: bool,
}

impl RuntimeTarget {
    fn many(target_triples: &[String]) -> Result<Vec<Self>, String> {
        if target_triples.is_empty() {
            return Ok(vec![Self {
                triple: host_runtime_target(),
                explicit: false,
            }]);
        }
        let mut seen = std::collections::BTreeSet::new();
        target_triples
            .iter()
            .map(|target| {
                crate::validate_runtime_target(target)?;
                if !seen.insert(target.clone()) {
                    return Err(format!("duplicate runtime target: {target}"));
                }
                Ok(Self {
                    triple: target.clone(),
                    explicit: true,
                })
            })
            .collect()
    }

    fn triple(&self) -> &str {
        &self.triple
    }

    fn build_output_root(&self, installation: &Path, project: &str, profile: &str) -> PathBuf {
        let cargo_profile = if profile == "dev" { "debug" } else { profile };
        let root = installation.join("output").join(profile).join(project);
        if self.explicit {
            root.join(self.triple()).join(cargo_profile)
        } else {
            root.join(cargo_profile)
        }
    }

    fn binary_file_name(&self, binary: &str) -> String {
        if self.triple.contains("windows") && !binary.ends_with(".exe") {
            format!("{binary}.exe")
        } else {
            binary.to_owned()
        }
    }
}

fn promote_file(source: &Path, destination: &Path) -> Result<(), String> {
    let parent = destination.parent().ok_or_else(|| {
        format!(
            "binary destination has no parent: {}",
            destination.display()
        )
    })?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create '{}': {error}", parent.display()))?;
    let temporary = destination.with_extension(format!("tmp-{}", std::process::id()));
    if temporary.exists() {
        fs::remove_file(&temporary)
            .map_err(|error| format!("failed to clear '{}': {error}", temporary.display()))?;
    }
    fs::copy(source, &temporary).map_err(|error| {
        format!(
            "failed to copy '{}' to '{}': {error}",
            source.display(),
            temporary.display()
        )
    })?;
    if destination.exists() {
        fs::remove_file(destination).map_err(|error| {
            format!(
                "failed to replace installed binary '{}': {error}",
                destination.display()
            )
        })?;
    }
    fs::rename(&temporary, destination).map_err(|error| {
        format!(
            "failed to promote '{}' to '{}': {error}",
            source.display(),
            destination.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::RuntimeTarget;
    use std::path::Path;

    #[test]
    fn deployment_profiles_resolve_to_distinct_artifact_roots() {
        let target = RuntimeTarget {
            triple: "x86_64-unknown-linux-gnu".to_owned(),
            explicit: true,
        };
        let installation = Path::new("/vapor");
        assert_eq!(
            target.build_output_root(installation, "vapor", "dev"),
            Path::new("/vapor/output/dev/vapor/x86_64-unknown-linux-gnu/debug")
        );
        assert_eq!(
            target.build_output_root(installation, "vapor", "release"),
            Path::new("/vapor/output/release/vapor/x86_64-unknown-linux-gnu/release")
        );
    }
}
