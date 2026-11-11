//! Promotion of built artifacts into the Vapor installation.

use crate::{
    cross_toolchain,
    discovery::{EnvironmentPaths, ensure_contained},
    target::host_runtime_target,
    workspace::WorkspaceManifest,
};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// Promote configured artifacts for the host target.
pub fn promote(paths: &EnvironmentPaths, manifest: &WorkspaceManifest) -> Result<usize, String> {
    promote_for_targets(paths, manifest, &[])
}

/// Promote configured artifacts for one or more runtime targets.
///
/// An empty target list selects the host runtime triple. All targets use the
/// managed target-specific output layout.
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
        let _ = project;
        let target_root = installation
            .join("cache/build")
            .join(self.triple());
        // With `--target`, Cargo adds the target triple beneath CARGO_TARGET_DIR.
        // Vapor already partitions CARGO_TARGET_DIR by triple, so promotion
        // must follow that nested path. A host build without `--target` writes
        // directly beneath its target directory.
        let target_root = if self.explicit {
            target_root.join(self.triple())
        } else {
            target_root
        };
        target_root.join(cargo_profile)
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
    if destination.is_file() && files_are_identical(source, destination)? {
        return Ok(());
    }
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

fn files_are_identical(source: &Path, destination: &Path) -> Result<bool, String> {
    let source_metadata = fs::metadata(source)
        .map_err(|error| format!("failed to inspect '{}': {error}", source.display()))?;
    let destination_metadata = fs::metadata(destination).map_err(|error| {
        format!(
            "failed to inspect existing binary '{}': {error}",
            destination.display()
        )
    })?;
    if source_metadata.len() != destination_metadata.len() {
        return Ok(false);
    }

    let mut source_file = fs::File::open(source)
        .map_err(|error| format!("failed to read '{}': {error}", source.display()))?;
    let mut destination_file = fs::File::open(destination).map_err(|error| {
        format!(
            "failed to inspect existing binary '{}': {error}",
            destination.display()
        )
    })?;
    let mut source_buffer = [0; 64 * 1024];
    let mut destination_buffer = [0; 64 * 1024];
    loop {
        let source_read = source_file
            .read(&mut source_buffer)
            .map_err(|error| format!("failed to read '{}': {error}", source.display()))?;
        let destination_read = destination_file
            .read(&mut destination_buffer)
            .map_err(|error| {
                format!(
                    "failed to inspect existing binary '{}': {error}",
                    destination.display()
                )
            })?;
        if source_read != destination_read
            || source_buffer[..source_read] != destination_buffer[..destination_read]
        {
            return Ok(false);
        }
        if source_read == 0 {
            return Ok(true);
        }
    }
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
            Path::new("/vapor/cache/build/x86_64-unknown-linux-gnu/debug")
        );
        assert_eq!(
            target.build_output_root(installation, "vapor", "release"),
            Path::new("/vapor/cache/build/x86_64-unknown-linux-gnu/release")
        );
    }

    #[test]
    fn host_runtime_target_uses_the_target_specific_cargo_artifact_root() {
        let target = RuntimeTarget::many(&[])
            .expect("host target")
            .remove(0);
        assert_eq!(
            target.build_output_root(Path::new("/vapor"), "vapor", "dev"),
            Path::new("/vapor/cache/build/x86_64-pc-windows-gnullvm/debug")
        );
    }
}
