//! Managed validation, release builds, and installation promotion.

/// Promotion of already-built application artifacts into the local installation.
pub mod promotion;

use crate::{
    cross_toolchain,
    discovery::{EnvironmentPaths, ensure_contained},
    managed_cargo::ApplicationCargo,
    process_output,
    target::host_runtime_target,
    workspace::{CargoProject, WorkspaceManifest},
};
use std::{
    fs,
    path::{Path, PathBuf},
};
use vapor_core::OperationOutputStream;

/// Summary of the Rust work completed before an application release is staged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseBuildReport {
    /// Number of installable Cargo workspaces validated and built.
    pub projects: usize,
    /// Number of target-specific binaries and libraries promoted into the app root.
    pub promoted_files: usize,
}

/// Validate, build, and promote the complete managed application target matrix.
///
/// Cargo, Rustc, Rustdoc, and cross-linkers are resolved from the Vapor
/// installation. Cargo subprocess output is forwarded to stderr so a caller
/// using stdout as an IPC transport remains protocol-safe.
pub fn build_application_release(
    paths: &EnvironmentPaths,
    runtime_targets: &[String],
) -> Result<ReleaseBuildReport, String> {
    build_application_release_with_events(
        paths,
        runtime_targets,
        |_, _, _| {},
        |_, text| {
            eprintln!("{text}");
        },
    )
}

/// Build a release while reporting completed steps and bounded child output.
pub fn build_application_release_with_events(
    paths: &EnvironmentPaths,
    runtime_targets: &[String],
    mut report_progress: impl FnMut(usize, usize, &str),
    mut report_output: impl FnMut(OperationOutputStream, &str),
) -> Result<ReleaseBuildReport, String> {
    if runtime_targets.is_empty() {
        return Err("application release needs at least one runtime target".to_owned());
    }
    let mut seen = std::collections::BTreeSet::new();
    for target in runtime_targets {
        crate::target::validate_runtime_target(target)?;
        if !seen.insert(target) {
            return Err(format!("duplicate runtime target: {target}"));
        }
    }

    let manifest = WorkspaceManifest::load(paths)?;
    let projects = manifest
        .cargo_projects()
        .iter()
        .filter(|project| !project.binaries().is_empty())
        .collect::<Vec<_>>();
    if projects.is_empty() {
        return Err(format!(
            "source root '{}' declares no installable Cargo workspaces",
            manifest.id()
        ));
    }

    let toolchain = ApplicationCargo::discover(paths)?;

    const STEPS: [BuildStep; 6] = [
        BuildStep::Fmt,
        BuildStep::Check,
        BuildStep::Test,
        BuildStep::Clippy,
        BuildStep::Doc,
        BuildStep::Release,
    ];
    let total_steps = projects.len() * runtime_targets.len() * STEPS.len();
    let mut completed_steps = 0;
    for project in &projects {
        for target in runtime_targets {
            for step in STEPS {
                run_step(paths, &toolchain, project, target, step, &mut report_output)?;
                completed_steps += 1;
                report_progress(
                    completed_steps,
                    total_steps,
                    &format!("{} {} for {target}", project.name(), step.label(target)),
                );
            }
        }
    }

    let promoted_files = promote_release_outputs(paths, &projects, runtime_targets)?;
    Ok(ReleaseBuildReport {
        projects: projects.len(),
        promoted_files,
    })
}

#[derive(Clone, Copy)]
enum BuildStep {
    Fmt,
    Check,
    Test,
    Clippy,
    Doc,
    Release,
}

impl BuildStep {
    fn label(self, target: &str) -> &'static str {
        match self {
            Self::Fmt => "fmt --check",
            Self::Check => "check",
            Self::Test if target == host_runtime_target() => "test",
            Self::Test => "test --no-run",
            Self::Clippy => "clippy",
            Self::Doc => "doc",
            Self::Release => "build --release",
        }
    }

    fn cargo_args(self, target: &str) -> Vec<&'static str> {
        match self {
            Self::Fmt => vec!["fmt", "--all"],
            Self::Check => vec!["check", "--workspace", "--all-targets", "--locked"],
            Self::Test if target == host_runtime_target() => {
                vec!["test", "--workspace", "--all-targets", "--locked"]
            }
            Self::Test => vec![
                "test",
                "--workspace",
                "--all-targets",
                "--locked",
                "--no-run",
            ],
            Self::Clippy => vec!["clippy", "--workspace", "--all-targets", "--locked"],
            Self::Doc => vec!["doc", "--workspace", "--no-deps", "--locked"],
            Self::Release => vec!["build", "--workspace", "--locked", "--release"],
        }
    }
}

fn cargo_target_args(target: &str) -> Vec<&str> {
    if target == host_runtime_target() {
        Vec::new()
    } else {
        vec!["--target", target]
    }
}

fn run_step(
    paths: &EnvironmentPaths,
    toolchain: &ApplicationCargo,
    project: &CargoProject,
    target: &str,
    step: BuildStep,
    report_output: &mut impl FnMut(OperationOutputStream, &str),
) -> Result<(), String> {
    let installation = paths.installation();
    let source_manifest = paths.source().root().join(project.manifest());
    let working_directory = source_manifest.parent().ok_or_else(|| {
        format!(
            "Cargo manifest has no parent: {}",
            source_manifest.display()
        )
    })?;
    let output_profile = if matches!(step, BuildStep::Release) {
        "release"
    } else {
        "dev"
    };
    let target_directory = installation
        .root()
        .join("output")
        .join(output_profile)
        .join(project.name());
    let mut command = toolchain.command();
    command
        .args(step.cargo_args(target))
        .arg("--manifest-path")
        .arg(&source_manifest)
        .current_dir(working_directory)
        .env("CARGO_TARGET_DIR", target_directory);
    if matches!(step, BuildStep::Fmt) {
        command.args(["--", "--check"]);
    } else if target != host_runtime_target() {
        cross_toolchain::configure_linker_env(&mut command, installation.root(), target)?;
        command.args(cargo_target_args(target));
    }
    if matches!(step, BuildStep::Clippy) {
        command.args(["--", "-D", "warnings"]);
    }
    if matches!(step, BuildStep::Doc) {
        command.env("RUSTDOCFLAGS", "-D warnings");
    }
    eprintln!("==> {}: {}", project.name(), step.label(target));
    let status = process_output::run(&mut command, report_output).map_err(|error| {
        format!(
            "failed to run {} for '{}': {error}",
            step.label(target),
            project.name()
        )
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} failed for project '{}' target '{target}' with {}",
            step.label(target),
            project.name(),
            status
        ))
    }
}

fn promote_release_outputs(
    paths: &EnvironmentPaths,
    projects: &[&CargoProject],
    runtime_targets: &[String],
) -> Result<usize, String> {
    let installation = paths.installation().root();
    let mut promoted = 0;
    for project in projects {
        for target in runtime_targets {
            let output_root = release_output_root(installation, project.name(), target);
            for binary in project.binaries() {
                let name = if target.contains("windows") && !binary.ends_with(".exe") {
                    format!("{binary}.exe")
                } else {
                    binary.clone()
                };
                let source = output_root.join(&name);
                ensure_contained(installation, &source)?;
                if !source.is_file() {
                    return Err(format!(
                        "release binary for '{}' target '{target}' is missing: {}",
                        project.name(),
                        source.display()
                    ));
                }
                let destination = installation.join("bin").join(target).join(&name);
                promote_file(&source, &destination)?;
                let parent = destination.parent().ok_or_else(|| {
                    format!("promoted binary has no parent: {}", destination.display())
                })?;
                promoted +=
                    cross_toolchain::copy_windows_runtime_dlls(installation, target, parent)?;
                promoted += 1;
            }
            for library in project.libraries(target) {
                let source = output_root.join(library);
                ensure_contained(installation, &source)?;
                if !source.is_file() {
                    return Err(format!(
                        "release runtime library for '{}' target '{target}' is missing: {}",
                        project.name(),
                        source.display()
                    ));
                }
                promote_file(
                    &source,
                    &installation.join("bin").join(target).join(library),
                )?;
                promoted += 1;
            }
        }
    }
    Ok(promoted)
}

fn release_output_root(installation: &Path, project: &str, target: &str) -> PathBuf {
    let root = installation.join("output/release").join(project);
    if target == host_runtime_target() {
        root.join("release")
    } else {
        root.join(target).join("release")
    }
}

fn promote_file(source: &Path, destination: &Path) -> Result<(), String> {
    let parent = destination
        .parent()
        .ok_or_else(|| format!("destination has no parent: {}", destination.display()))?;
    fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let temporary = destination.with_extension(format!("tmp-{}", std::process::id()));
    if temporary.exists() {
        fs::remove_file(&temporary).map_err(|error| format!("{}: {error}", temporary.display()))?;
    }
    fs::copy(source, &temporary).map_err(|error| {
        format!(
            "failed to copy '{}' to '{}': {error}",
            source.display(),
            temporary.display()
        )
    })?;
    let previous = destination.with_extension(format!("previous-{}", std::process::id()));
    if previous.exists() {
        fs::remove_file(&previous).map_err(|error| format!("{}: {error}", previous.display()))?;
    }
    let had_previous = destination.exists();
    if had_previous {
        fs::rename(destination, &previous).map_err(|error| {
            format!(
                "failed to preserve current '{}': {error}",
                destination.display()
            )
        })?;
    }
    if let Err(error) = fs::rename(&temporary, destination) {
        if had_previous {
            let _ = fs::rename(&previous, destination);
        }
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "failed to promote '{}' to '{}': {error}",
            source.display(),
            destination.display()
        ));
    }
    if had_previous {
        fs::remove_file(&previous).map_err(|error| {
            format!(
                "promoted '{}', but could not remove backup '{}': {error}",
                destination.display(),
                previous.display()
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod target_tests {
    use super::{cargo_target_args, release_output_root};
    use std::path::Path;

    #[test]
    fn release_build_omits_the_target_flag_for_the_host_triple() {
        let host = crate::host_runtime_target();
        assert!(cargo_target_args(&host).is_empty());
    }

    #[test]
    fn release_build_keeps_foreign_target_selection() {
        assert_eq!(
            cargo_target_args("x86_64-pc-windows-gnullvm"),
            ["--target", "x86_64-pc-windows-gnullvm"]
        );
    }

    #[test]
    fn release_promotion_uses_default_layout_for_host_and_triple_layout_for_cross_target() {
        let installation = Path::new("/vapor");
        let host = crate::host_runtime_target();
        assert_eq!(
            release_output_root(installation, "vapor", &host),
            Path::new("/vapor/output/release/vapor/release")
        );
        assert_eq!(
            release_output_root(installation, "vapor", "x86_64-pc-windows-gnullvm"),
            Path::new("/vapor/output/release/vapor/x86_64-pc-windows-gnullvm/release")
        );
    }
}
