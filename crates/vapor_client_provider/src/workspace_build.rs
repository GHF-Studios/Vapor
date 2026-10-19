//! Vapor-managed source workspace build and validation operations.

use crate::{
    cross_toolchain,
    discovery::EnvironmentPaths,
    process_output,
    release::managed_path,
    target::{host_runtime_target, validate_runtime_target},
    workspace::{CargoProject, WorkspaceManifest},
};
use std::{env, process::Command};
use vapor_client_domain::WorkspaceProjectSelection;
use vapor_core::OperationOutputStream;

/// Build policy for an authored workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildMode {
    /// Apply Rustfmt to all Cargo projects.
    Format,
    /// Type-check all workspace targets.
    Check,
    /// Run host tests or compile tests for a foreign target.
    Test,
    /// Build all Cargo projects using the development profile.
    Build,
    /// Build all Cargo projects using the optimized release profile.
    BuildRelease,
    /// Check formatting, compile, test, lint, and build documentation.
    Validate,
    /// Run formatting, checks, tests, lint, docs, then build using the development profile.
    ValidateAndBuild,
}

/// Summary of a managed source workspace build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildReport {
    /// Number of Cargo workspaces processed.
    pub projects: usize,
    /// Runtime targets built; an empty list means the host-default target.
    pub runtime_targets: Vec<String>,
}

/// Run a managed build or a complete publication validation/build sequence.
///
/// Development artifacts remain in `output/dev`, release builds in
/// `output/release`; no operation promotes binaries into the installed app
/// root. Empty target lists preserve Cargo's host-default output layout.
///
/// # Errors
///
/// Returns an error if the source workspace is invalid, the managed toolchain
/// is incomplete, a target is invalid, or any Cargo step fails.
pub fn run(
    paths: &EnvironmentPaths,
    mode: BuildMode,
    runtime_targets: &[String],
) -> Result<BuildReport, String> {
    run_selected(
        paths,
        mode,
        &WorkspaceProjectSelection::All,
        runtime_targets,
    )
}

/// Run a managed workspace operation for a declared subset of Cargo projects.
pub fn run_selected(
    paths: &EnvironmentPaths,
    mode: BuildMode,
    selection: &WorkspaceProjectSelection,
    runtime_targets: &[String],
) -> Result<BuildReport, String> {
    run_selected_with_progress(paths, mode, selection, runtime_targets, |_, _, _| {})
}

/// Run a selected managed workspace operation and report completed Cargo steps.
pub fn run_selected_with_progress(
    paths: &EnvironmentPaths,
    mode: BuildMode,
    selection: &WorkspaceProjectSelection,
    runtime_targets: &[String],
    report_progress: impl FnMut(usize, usize, &str),
) -> Result<BuildReport, String> {
    run_selected_with_events(
        paths,
        mode,
        selection,
        runtime_targets,
        report_progress,
        process_output::write_to_terminal,
    )
}

/// Run a selected managed workspace operation and forward its child output.
pub fn run_selected_with_events(
    paths: &EnvironmentPaths,
    mode: BuildMode,
    selection: &WorkspaceProjectSelection,
    runtime_targets: &[String],
    report_progress: impl FnMut(usize, usize, &str),
    report_output: impl FnMut(OperationOutputStream, &str),
) -> Result<BuildReport, String> {
    run_selected_with_events_cancellable(
        paths,
        mode,
        selection,
        runtime_targets,
        report_progress,
        || false,
        report_output,
    )
}

/// Run a selected managed workspace operation with cooperative cancellation.
pub fn run_selected_with_events_cancellable(
    paths: &EnvironmentPaths,
    mode: BuildMode,
    selection: &WorkspaceProjectSelection,
    runtime_targets: &[String],
    report_progress: impl FnMut(usize, usize, &str),
    mut is_cancelled: impl FnMut() -> bool,
    mut report_output: impl FnMut(OperationOutputStream, &str),
) -> Result<BuildReport, String> {
    validate_targets(runtime_targets)?;
    if is_cancelled() {
        return Err("operation cancelled".to_owned());
    }
    let manifest = WorkspaceManifest::load(paths)?;
    let projects = match selection {
        WorkspaceProjectSelection::All => manifest.cargo_projects().iter().collect::<Vec<_>>(),
        WorkspaceProjectSelection::Installable => manifest
            .cargo_projects()
            .iter()
            .filter(|project| !project.binaries().is_empty())
            .collect::<Vec<_>>(),
        WorkspaceProjectSelection::Named(name) => manifest
            .cargo_projects()
            .iter()
            .filter(|project| project.name() == name)
            .collect::<Vec<_>>(),
    };
    if projects.is_empty() {
        return Err(format!(
            "source root '{}' has no Cargo workspaces matching the selected project scope",
            manifest.id(),
        ));
    }
    let targets = if runtime_targets.is_empty() {
        vec![None]
    } else {
        runtime_targets
            .iter()
            .map(|target| Some(target.as_str()))
            .collect()
    };
    let steps = match mode {
        BuildMode::Format => vec![CargoStep::Format],
        BuildMode::Check => vec![CargoStep::Check],
        BuildMode::Test => vec![CargoStep::Test],
        BuildMode::Build => vec![CargoStep::Build],
        BuildMode::BuildRelease => vec![CargoStep::BuildRelease],
        BuildMode::Validate => validation_steps().to_vec(),
        BuildMode::ValidateAndBuild => validation_steps_with_build().to_vec(),
    };
    run_plan_steps(
        projects.len(),
        targets.len(),
        steps.len(),
        |project, target, step| {
            if is_cancelled() {
                return Err("operation cancelled".to_owned());
            }
            let project = projects[project];
            let step = steps[step];
            run_step(
                paths,
                project,
                step,
                targets[target],
                &mut is_cancelled,
                &mut report_output,
            )?;
            let target = targets[target]
                .map(|target| format!(" for {target}"))
                .unwrap_or_default();
            Ok(format!(
                "Finished {} for {}{target}",
                step.label(),
                project.name()
            ))
        },
        report_progress,
    )?;
    Ok(BuildReport {
        projects: projects.len(),
        runtime_targets: runtime_targets.to_vec(),
    })
}

fn run_plan_steps(
    project_count: usize,
    target_count: usize,
    step_count: usize,
    mut run_step: impl FnMut(usize, usize, usize) -> Result<String, String>,
    mut report_progress: impl FnMut(usize, usize, &str),
) -> Result<(), String> {
    let total = project_count * target_count * step_count;
    let mut completed = 0;
    for project in 0..project_count {
        for target in 0..target_count {
            for step in 0..step_count {
                let message = run_step(project, target, step)?;
                completed += 1;
                report_progress(completed, total, &message);
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CargoStep {
    Format,
    FmtCheck,
    Check,
    Test,
    Clippy,
    Doc,
    Build,
    BuildRelease,
}

impl CargoStep {
    fn label(self) -> &'static str {
        match self {
            Self::Format => "formatting",
            Self::FmtCheck => "format check",
            Self::Check => "check",
            Self::Test => "tests",
            Self::Clippy => "Clippy",
            Self::Doc => "documentation",
            Self::Build => "development build",
            Self::BuildRelease => "release build",
        }
    }
}

fn validation_steps() -> &'static [CargoStep] {
    &[
        CargoStep::FmtCheck,
        CargoStep::Check,
        CargoStep::Test,
        CargoStep::Clippy,
        CargoStep::Doc,
    ]
}

fn validation_steps_with_build() -> &'static [CargoStep] {
    &[
        CargoStep::FmtCheck,
        CargoStep::Check,
        CargoStep::Test,
        CargoStep::Clippy,
        CargoStep::Doc,
        CargoStep::Build,
    ]
}

fn cargo_args(step: CargoStep, target: Option<&str>) -> Vec<String> {
    let mut args = match step {
        CargoStep::Format | CargoStep::FmtCheck => vec!["fmt", "--all"],
        CargoStep::Check => vec!["check", "--workspace", "--all-targets", "--locked"],
        CargoStep::Test => {
            let mut args = vec!["test", "--workspace", "--all-targets", "--locked"];
            if target.is_some_and(|target| target != host_runtime_target()) {
                args.push("--no-run");
            }
            args
        }
        CargoStep::Clippy => vec!["clippy", "--workspace", "--all-targets", "--locked"],
        CargoStep::Doc => vec!["doc", "--workspace", "--no-deps", "--locked"],
        CargoStep::Build => vec!["build", "--workspace", "--locked"],
        CargoStep::BuildRelease => vec!["build", "--workspace", "--locked", "--release"],
    }
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if !matches!(step, CargoStep::Format | CargoStep::FmtCheck)
        && let Some(target) = target
    {
        args.extend(["--target".to_owned(), target.to_owned()]);
    }
    args
}

fn cargo_invocation_args(
    step: CargoStep,
    target: Option<&str>,
    manifest: &std::path::Path,
) -> Vec<std::ffi::OsString> {
    let mut args = cargo_args(step, target)
        .into_iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>();
    args.push("--manifest-path".into());
    args.push(manifest.as_os_str().to_owned());
    match step {
        CargoStep::FmtCheck => args.extend(["--".into(), "--check".into()]),
        CargoStep::Clippy => args.extend(["--".into(), "-D".into(), "warnings".into()]),
        _ => {}
    }
    args
}

fn run_step(
    paths: &EnvironmentPaths,
    project: &CargoProject,
    step: CargoStep,
    target: Option<&str>,
    is_cancelled: &mut impl FnMut() -> bool,
    report_output: &mut impl FnMut(OperationOutputStream, &str),
) -> Result<(), String> {
    let cargo = paths
        .installation()
        .bundled_cargo()
        .ok_or_else(|| "Steam installation has no bundled Cargo executable".to_owned())?;
    let toolchain_bin = cargo
        .parent()
        .ok_or_else(|| format!("managed Cargo has no parent: {}", cargo.display()))?;
    let toolchain_name = toolchain_bin
        .parent()
        .and_then(std::path::Path::file_name)
        .ok_or_else(|| "managed Rust toolchain has no stable directory name".to_owned())?
        .to_string_lossy()
        .into_owned();
    let rustc = toolchain_bin.join(executable("rustc"));
    let rustdoc = toolchain_bin.join(executable("rustdoc"));
    if !rustc.is_file() || !rustdoc.is_file() {
        return Err(format!(
            "managed Rust toolchain is incomplete beside {}",
            cargo.display()
        ));
    }
    let source_manifest = paths.source().root().join(project.manifest());
    let working_directory = source_manifest.parent().ok_or_else(|| {
        format!(
            "Cargo manifest has no parent: {}",
            source_manifest.display()
        )
    })?;
    let installation = paths.installation().root();
    let profile = if step == CargoStep::BuildRelease {
        "release"
    } else {
        "dev"
    };
    let target_directory = installation
        .join("output")
        .join(profile)
        .join(project.name());
    let mut command = Command::new(&cargo);
    command
        .args(cargo_invocation_args(step, target, &source_manifest))
        .current_dir(working_directory)
        .env("VAPOR_HOME", installation)
        .env("CARGO_HOME", installation.join("cargo-home"))
        .env("RUSTUP_HOME", installation.join("rustup-home"))
        .env("RUSTUP_TOOLCHAIN", &toolchain_name)
        .env("RUSTC", &rustc)
        .env("RUSTDOC", &rustdoc)
        .env("CARGO_TARGET_DIR", target_directory)
        .env("PATH", managed_path(paths, toolchain_bin)?)
        .env_remove("RUSTC_WRAPPER");
    if !matches!(step, CargoStep::Format | CargoStep::FmtCheck)
        && let Some(target) = target
    {
        cross_toolchain::configure_linker_env(&mut command, installation, target)?;
    }
    if step == CargoStep::Doc {
        command.env("RUSTDOCFLAGS", "-D warnings");
    }
    eprintln!("==> {}: {}", project.name(), step_label(step, target));
    let status = process_output::run_cancellable(&mut command, is_cancelled, report_output)
        .map_err(|error| {
            format!(
                "failed to run {} for '{}': {error}",
                step_label(step, target),
                project.name()
            )
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} failed for project '{}' with {status}",
            step_label(step, target),
            project.name()
        ))
    }
}

fn step_label(step: CargoStep, target: Option<&str>) -> &'static str {
    match step {
        CargoStep::Format => "fmt",
        CargoStep::FmtCheck => "fmt --check",
        CargoStep::Check => "check",
        CargoStep::Test if target.is_none_or(|target| target == host_runtime_target()) => "test",
        CargoStep::Test => "test --no-run",
        CargoStep::Clippy => "clippy",
        CargoStep::Doc => "doc",
        CargoStep::Build => "build",
        CargoStep::BuildRelease => "build --release",
    }
}

fn validate_targets(targets: &[String]) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    for target in targets {
        validate_runtime_target(target)?;
        if !seen.insert(target) {
            return Err(format!("duplicate runtime target: {target}"));
        }
    }
    Ok(())
}

fn executable(name: &str) -> String {
    format!("{name}{}", env::consts::EXE_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::{
        CargoStep, cargo_args, cargo_invocation_args, run_plan_steps, validation_steps,
        validation_steps_with_build,
    };

    #[test]
    fn workspace_plan_reports_progress_after_each_successful_step() {
        let mut calls = Vec::new();
        let mut progress = Vec::new();

        run_plan_steps(
            2,
            2,
            3,
            |project, target, step| {
                calls.push((project, target, step));
                Ok(format!("step-{project}-{target}-{step}"))
            },
            |completed, total, label| progress.push((completed, total, label.to_owned())),
        )
        .unwrap();

        assert_eq!(calls.len(), 12);
        assert_eq!(calls[0], (0, 0, 0));
        assert_eq!(calls[11], (1, 1, 2));
        assert_eq!(progress.first(), Some(&(1, 12, "step-0-0-0".to_owned())));
        assert_eq!(progress.last(), Some(&(12, 12, "step-1-1-2".to_owned())));
        assert!(progress.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    #[test]
    fn workspace_plan_stops_progress_when_a_step_fails() {
        let mut progress = Vec::new();

        let result = run_plan_steps(
            2,
            1,
            2,
            |project, _, step| {
                if project == 0 && step == 1 {
                    Err("step failed".to_owned())
                } else {
                    Ok(format!("step-{project}-{step}"))
                }
            },
            |completed, total, label| progress.push((completed, total, label.to_owned())),
        );

        assert_eq!(result.unwrap_err(), "step failed");
        assert_eq!(progress, [(1, 4, "step-0-0".to_owned())]);
    }

    #[test]
    fn format_check_keeps_manifest_selection_before_rustfmt_arguments() {
        assert_eq!(
            cargo_invocation_args(
                CargoStep::FmtCheck,
                None,
                std::path::Path::new("Cargo.toml")
            ),
            [
                "fmt",
                "--all",
                "--manifest-path",
                "Cargo.toml",
                "--",
                "--check"
            ]
            .map(std::ffi::OsString::from)
        );
    }

    #[test]
    fn clippy_keeps_manifest_selection_before_driver_arguments() {
        assert_eq!(
            cargo_invocation_args(CargoStep::Clippy, None, std::path::Path::new("Cargo.toml")),
            [
                "clippy",
                "--workspace",
                "--all-targets",
                "--locked",
                "--manifest-path",
                "Cargo.toml",
                "--",
                "-D",
                "warnings"
            ]
            .map(std::ffi::OsString::from)
        );
    }

    #[test]
    fn workspace_format_action_runs_rustfmt_without_check_only_mode() {
        assert_eq!(cargo_args(CargoStep::Format, None), ["fmt", "--all"]);
    }

    #[test]
    fn build_does_not_select_release_or_cross_target_outputs_on_host() {
        assert_eq!(
            cargo_args(CargoStep::Build, None),
            ["build", "--workspace", "--locked"]
        );
    }

    #[test]
    fn release_build_uses_cargo_release_profile() {
        assert_eq!(
            cargo_args(CargoStep::BuildRelease, None),
            ["build", "--workspace", "--locked", "--release"]
        );
    }

    #[test]
    fn cross_target_tests_compile_without_trying_to_run_foreign_binaries() {
        assert_eq!(
            cargo_args(CargoStep::Test, Some("x86_64-pc-windows-gnullvm")),
            [
                "test",
                "--workspace",
                "--all-targets",
                "--locked",
                "--no-run",
                "--target",
                "x86_64-pc-windows-gnullvm"
            ]
        );
    }

    #[test]
    fn publication_validation_builds_in_order_without_promoting_outputs() {
        assert_eq!(
            validation_steps_with_build(),
            [
                CargoStep::FmtCheck,
                CargoStep::Check,
                CargoStep::Test,
                CargoStep::Clippy,
                CargoStep::Doc,
                CargoStep::Build
            ]
        );
    }

    #[test]
    fn validation_workflow_does_not_build_or_promote_outputs() {
        assert_eq!(
            validation_steps(),
            [
                CargoStep::FmtCheck,
                CargoStep::Check,
                CargoStep::Test,
                CargoStep::Clippy,
                CargoStep::Doc,
            ]
        );
    }
}
