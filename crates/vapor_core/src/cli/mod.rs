//! Command-line projections over Vapor Core.
//!
//! Executables are intentionally thin. The universal Vapor CLI and dedicated
//! application CLIs expose subsets of the same underlying core operations.

mod commands;
mod platform_activity;

use crate::{
    CargoDependencyState, CargoPackageInspection, ContentKind, ContentVersionId,
    DevelopmentOperation, LibraryCargoReconciliation, LocalCatalog, LocalContent, ManagedToolchain,
    ResolvedComposition, ResolvedContentGraph, SteamDeploymentOptions, UninstallOptions, VaporId,
    VaporInstallation, VaporProject, VaporRole, VaporSuperworkspace, VaporWorkspace,
    SUPERWORKSPACE_MANIFEST_FILE_NAME, acquire_ecosystem_repositories, build_cargo_realization,
    demote_role, deploy_ecosystem_to_steam, deploy_workspace, development_target_dir,
    diagnose_managed_state, discover_local_content, forget_source, generate_local_cargo_realization,
    git_available, inspect_local_cargo_package, install_installation, promote_role,
    reconcile_existing_development_environment, repair_local_library_cargo_dependencies,
    repair_managed_state, resolve_local_content_kind, resolve_local_packagepack,
    resolve_source_context, role_status, run_cargo_realization, run_workspace_operation,
    source_state, uninstall_installation, verify_local_library_cargo_dependencies,
};
use crate::install::ensure_canonical_superworkspace;
use clap::Parser;
use commands::*;
use platform_activity::*;
use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliSurface {
    Vapor,
    Installer,
}

pub fn run_cli(surface: CliSurface) -> ExitCode {
    match surface {
        CliSurface::Vapor => {
            finish_parse(VaporCli::try_parse().map(|cli| execute_vapor(cli.command)))
        }

        CliSurface::Installer => {
            finish_parse(InstallerCli::try_parse().map(|cli| execute_installer(cli.command)))
        }
    }
}

fn finish_parse(result: Result<Result<(), String>, clap::Error>) -> ExitCode {
    match result {
        Ok(Ok(())) => ExitCode::SUCCESS,

        Ok(Err(error)) => {
            eprintln!("error: {error}");

            ExitCode::FAILURE
        }

        Err(error) => {
            let exit_code = error.exit_code();

            let _ = error.print();

            ExitCode::from(exit_code as u8)
        }
    }
}

fn execute_vapor(command: VaporCommand) -> Result<(), String> {
    match command {
        VaporCommand::Installation { command } => execute_installation(command),

        VaporCommand::Role { command } => execute_role(command),

        VaporCommand::Authority { command } => execute_authority(command),

        VaporCommand::Toolchain { command } => execute_toolchain(command),

        VaporCommand::Source { command } => execute_source(command),

        VaporCommand::Client { command } => execute_client(command),

        VaporCommand::PlatformServer { command } => execute_platform_server(command),

        VaporCommand::Packagepack { command } => execute_packagepack(command),

        VaporCommand::Enginepack { command } => {
            execute_graph_content(ContentKind::Enginepack, command)
        }

        VaporCommand::Gamepack { command } => execute_graph_content(ContentKind::Gamepack, command),

        VaporCommand::Modpack { command } => execute_graph_content(ContentKind::Modpack, command),

        VaporCommand::Engine { command } => execute_behavioral(ContentKind::Engine, command),

        VaporCommand::Game { command } => execute_behavioral(ContentKind::Game, command),

        VaporCommand::EngineMod { command } => execute_behavioral(ContentKind::EngineMod, command),

        VaporCommand::GameMod { command } => execute_behavioral(ContentKind::GameMod, command),

        VaporCommand::ExtensionMod { command } => {
            execute_behavioral(ContentKind::ExtensionMod, command)
        }

        VaporCommand::Library { command } => execute_library(command),
    }
}

fn execute_installer(command: InstallerCommand) -> Result<(), String> {
    match command {
        InstallerCommand::Install => installer_install(),

        InstallerCommand::Installation { command } => execute_installation(command),

        InstallerCommand::Role { command } => execute_role(command),

        InstallerCommand::Authority { command } => execute_authority(command),

        InstallerCommand::Toolchain { command } => execute_toolchain(command),

        InstallerCommand::Uninstall(args) => installer_uninstall(args),
    }
}

fn installer_install() -> Result<(), String> {
    let report = install_installation().map_err(|error| error.to_string())?;
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;
    let role = role_status(&installation).map_err(|error| error.to_string())?;

    println!("Vapor Installation installed:");
    println!("  installation: {}", report.installation_root.display());
    println!("  user data: {}", report.user_data_root.display());
    println!("  superworkspace: {}", report.superworkspace_root.display());
    println!("  role: {}", role.installed_role);

    if report.integration.is_empty() {
        println!("  integration: already current");
    } else {
        println!("  integration:");
        for item in &report.integration {
            println!("    {item}");
        }
    }

    println!("  note: reopen existing shells to inherit updated environment");
    Ok(())
}

fn installer_uninstall(args: UninstallArgs) -> Result<(), String> {
    let destructive = args.purge_app_external || args.purge_superworkspace;

    if destructive && !args.yes && !confirm_uninstall_purge(&args)? {
        println!("Uninstall cancelled.");
        return Ok(());
    }

    let report = uninstall_installation(UninstallOptions {
        purge_app_external: args.purge_app_external,
        purge_superworkspace: args.purge_superworkspace,
    })
    .map_err(|error| error.to_string())?;

    println!("Vapor uninstall");
    println!("  installation: {}", report.installation_root.display());
    println!("  user data:    {}", report.user_data_root.display());

    if let Some(superworkspace) = &report.superworkspace_root {
        println!("  superworkspace: {}", superworkspace.display());
    }

    if !report.legacy_app_state_removed.is_empty() {
        println!("  removed legacy App Instance mutable state:");
        for path in &report.legacy_app_state_removed {
            println!("    {}", path.display());
        }
    }

    if report.integration.is_empty() {
        println!("  integration: already absent");
    } else {
        println!("  integration:");
        for item in &report.integration {
            println!("    {item}");
        }
    }

    if args.purge_app_external {
        println!(
            "  app external data: {}",
            if report.app_external_purged {
                "purged"
            } else {
                "already absent"
            }
        );
    } else {
        println!("  app external data: preserved");
    }

    if args.purge_superworkspace {
        println!(
            "  superworkspace: {}",
            if report.superworkspace_purged {
                "purged"
            } else {
                "already absent"
            }
        );
    } else {
        println!("  authored source: preserved");
    }

    println!("  note: already-running shells keep inherited environment until restarted");

    Ok(())
}

fn confirm_uninstall_purge(args: &UninstallArgs) -> Result<bool, String> {
    eprintln!("WARNING: the requested uninstall includes destructive purge operations.");

    if args.purge_app_external {
        eprintln!("  - permanently delete Vapor-owned OS user data and caches");
    }

    if args.purge_superworkspace {
        eprintln!("  - permanently delete the active Superworkspace and authored source");
    }

    eprint!("Type `yes` to continue: ");
    io::stderr()
        .flush()
        .map_err(|error| format!("failed to write confirmation prompt: {error}"))?;

    let mut confirmation = String::new();
    io::stdin()
        .read_line(&mut confirmation)
        .map_err(|error| format!("failed to read confirmation: {error}"))?;

    Ok(confirmation.trim().eq_ignore_ascii_case("yes"))
}

fn execute_installation(command: InstallationCommand) -> Result<(), String> {
    match command {
        InstallationCommand::Status => installation_status(),

        InstallationCommand::Diagnose => installation_diagnose(),

        InstallationCommand::Repair => installation_repair(),
    }
}

fn execute_role(command: RoleCommand) -> Result<(), String> {
    match command {
        RoleCommand::Status => installer_role_status(),

        RoleCommand::Promote { role } => installer_role_promote(role),

        RoleCommand::Demote { role } => installer_role_demote(role),
    }
}

fn execute_authority(command: AuthorityCommand) -> Result<(), String> {
    match command {
        AuthorityCommand::Status => authority_status(),
    }
}

fn execute_toolchain(command: ToolchainCommand) -> Result<(), String> {
    match command {
        ToolchainCommand::Status => toolchain_status(),

        ToolchainCommand::Install => toolchain_install(),

        ToolchainCommand::Diagnose => not_implemented("toolchain", "diagnose"),

        ToolchainCommand::Repair => not_implemented("toolchain", "repair"),

        ToolchainCommand::Cargo { project, args } => toolchain_cargo(project, args),
    }
}

fn toolchain_cargo(explicit_project: Option<String>, args: Vec<OsString>) -> Result<(), String> {
    let toolchain = ManagedToolchain::discover().map_err(|error| error.to_string())?;

    let project = resolve_toolchain_cargo_project(&toolchain, explicit_project.as_deref(), &args)?;

    let mut command = toolchain
        .cargo_command()
        .map_err(|error| error.to_string())?;

    command.args(&args);

    if let Some(project) = &project {
        command.current_dir(&project.root);
    }

    let status = command.status().map_err(|error| {
        format!(
            "failed to start Vapor-managed Cargo `{}`: {error}",
            toolchain.cargo_path.display()
        )
    })?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("Vapor-managed Cargo exited with {status}"))
    }
}

fn resolve_toolchain_cargo_project(
    toolchain: &ManagedToolchain,
    explicit_project: Option<&str>,
    args: &[OsString],
) -> Result<Option<VaporProject>, String> {
    let package_hints = cargo_package_hints(args);
    let explicit_manifest = cargo_has_manifest_path(args);

    let needs_project = explicit_project.is_some()
        || (!explicit_manifest
            && (!package_hints.is_empty() || cargo_needs_project_context(args)));

    if !needs_project {
        return Ok(None);
    }

    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let source = resolve_source_context(&installation, None).map_err(|error| error.to_string())?;

    let superworkspace = VaporSuperworkspace::discover_from(&source.root).map_err(|error| {
        format!(
            "failed to resolve Vapor Superworkspace from active source `{}`: {error}",
            source.root.display()
        )
    })?;

    if let Some(selector) = explicit_project {
        let matches = superworkspace
            .projects
            .iter()
            .filter(|candidate| {
                candidate.project.name == selector
                    || format!("{}/{}", candidate.repository, candidate.project.name,) == selector
            })
            .collect::<Vec<_>>();

        return match matches.as_slice() {
            [candidate] => Ok(Some(candidate.project.clone())),

            [] => Err(format!(
                "no Vapor Project `{selector}` exists in active Superworkspace `{}`; available Projects: {}",
                superworkspace.root.display(),
                cargo_project_labels(&superworkspace).join(", "),
            )),

            _ => Err(format!(
                "Vapor Project selector `{selector}` is ambiguous in active Superworkspace `{}`; use a repository-qualified selector such as `repository/project`",
                superworkspace.root.display(),
            )),
        };
    }

    if !package_hints.is_empty() {
        let mut matches = Vec::new();

        for candidate in &superworkspace.projects {
            let packages = cargo_project_packages(toolchain, &candidate.project)?;

            if package_hints
                .iter()
                .all(|hint| packages.iter().any(|package| package == hint))
            {
                matches.push(candidate);
            }
        }

        return match matches.as_slice() {
            [candidate] => Ok(Some(candidate.project.clone())),

            [] => Err(format!(
                "no Vapor Project in active Superworkspace `{}` contains all requested Cargo package(s): {}",
                superworkspace.root.display(),
                package_hints.join(", "),
            )),

            _ => Err(format!(
                "Cargo package selection {} matches multiple Vapor Projects in active Superworkspace `{}`; select one explicitly with `--project`",
                package_hints.join(", "),
                superworkspace.root.display(),
            )),
        };
    }

    let working_directory = env::current_dir()
        .map_err(|error| format!("failed to determine Cargo execution context: {error}"))?;

    if let Some(candidate) = superworkspace
        .projects
        .iter()
        .filter(|candidate| working_directory.starts_with(&candidate.project.root))
        .max_by_key(|candidate| candidate.project.root.components().count())
    {
        return Ok(Some(candidate.project.clone()));
    }

    if let [candidate] = superworkspace.projects.as_slice() {
        return Ok(Some(candidate.project.clone()));
    }

    Err(format!(
        "Cargo requires a Vapor Project context, but active Superworkspace `{}` contains multiple Projects and none was selected; use Cargo `-p/--package` or `vapor toolchain cargo --project <PROJECT> -- ...`; available Projects: {}",
        superworkspace.root.display(),
        cargo_project_labels(&superworkspace).join(", "),
    ))
}

fn cargo_project_labels(superworkspace: &VaporSuperworkspace) -> Vec<String> {
    superworkspace
        .projects
        .iter()
        .map(|candidate| format!("{}/{}", candidate.repository, candidate.project.name,))
        .collect()
}

fn cargo_package_hints(args: &[OsString]) -> Vec<String> {
    let mut hints = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let argument = args[index].to_string_lossy();

        if argument == "--" {
            break;
        }

        if argument == "-p" || argument == "--package" {
            if let Some(value) = args.get(index + 1) {
                hints.push(cargo_package_name(&value.to_string_lossy()).to_owned());

                index += 2;
                continue;
            }
        }

        if let Some(value) = argument.strip_prefix("--package=") {
            hints.push(cargo_package_name(value).to_owned());

            index += 1;
            continue;
        }

        if let Some(value) = argument
            .strip_prefix("-p")
            .filter(|value| !value.is_empty())
        {
            hints.push(cargo_package_name(value).to_owned());
        }

        index += 1;
    }

    hints.sort();
    hints.dedup();

    hints
}

fn cargo_has_manifest_path(args: &[OsString]) -> bool {
    let mut index = 0;

    while index < args.len() {
        let argument = args[index].to_string_lossy();

        if argument == "--" {
            break;
        }

        if argument == "--manifest-path" || argument.starts_with("--manifest-path=") {
            return true;
        }

        index += 1;
    }

    false
}

fn cargo_package_name(spec: &str) -> &str {
    spec.split_once('@').map(|(name, _)| name).unwrap_or(spec)
}

fn cargo_needs_project_context(args: &[OsString]) -> bool {
    let Some(first) = args.first() else {
        return false;
    };

    matches!(
        first.to_string_lossy().as_ref(),
        value
            if !matches!(
                value,
                "--version"
                    | "-V"
                    | "-vV"
                    | "version"
                    | "--help"
                    | "-h"
                    | "help"
            )
    )
}

fn cargo_project_packages(
    toolchain: &ManagedToolchain,
    project: &VaporProject,
) -> Result<Vec<String>, String> {
    #[derive(serde::Deserialize)]
    struct Metadata {
        packages: Vec<Package>,
    }

    #[derive(serde::Deserialize)]
    struct Package {
        name: String,
    }

    let output = toolchain
        .cargo_command()
        .map_err(|error| error.to_string())?
        .arg("metadata")
        .args(["--format-version", "1", "--no-deps"])
        .arg("--manifest-path")
        .arg(&project.cargo_manifest_path)
        .current_dir(&project.root)
        .output()
        .map_err(|error| {
            format!(
                "failed to inspect Cargo packages for Vapor Project `{}`: {error}",
                project.name,
            )
        })?;

    if !output.status.success() {
        return Err(format!(
            "Cargo metadata failed while inspecting Vapor Project `{}` with {}: {}",
            project.name,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }

    let metadata: Metadata = serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "Cargo returned invalid metadata for Vapor Project `{}`: {error}",
            project.name,
        )
    })?;

    Ok(metadata
        .packages
        .into_iter()
        .map(|package| package.name)
        .collect())
}

fn execute_source(command: SourceCommand) -> Result<(), String> {
    match command {
        SourceCommand::Status => source_status(),

        SourceCommand::List => source_list(),

        SourceCommand::Setup => source_setup(),

        SourceCommand::Acquire { source } => source_acquire(&source),

        SourceCommand::Remove { source, yes } => source_remove(&source, yes),

        SourceCommand::Teardown { yes } => source_teardown(yes),

        SourceCommand::Restore => source_restore(),
    }
}

fn source_setup() -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;
    let superworkspace =
        ensure_canonical_superworkspace(&installation).map_err(|error| error.to_string())?;
    let state = source_state(&installation).map_err(|error| error.to_string())?;

    println!("Vapor Superworkspace ready:");
    println!("  root: {}", superworkspace.root.display());
    println!(
        "  marker: {}",
        superworkspace
            .root
            .join(SUPERWORKSPACE_MANIFEST_FILE_NAME)
            .display()
    );
    println!("  source state: {}", state.state_path.display());

    if let Err(error) = synchronize_existing_development_environment(&superworkspace.root) {
        eprintln!("warning: Superworkspace was created, but development-state synchronization failed: {error}");
    }

    Ok(())
}

fn synchronize_existing_development_environment(source_root: &Path) -> Result<(), String> {
    let changed = reconcile_existing_development_environment(source_root)
        .map_err(|error| error.to_string())?;

    if !changed.is_empty() {
        println!("Synchronized existing Vapor development environment.");
    }

    Ok(())
}

fn source_restore() -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;
    let superworkspace = configured_superworkspace(&installation)?;

    println!("Restoring registered Vapor source...");

    let report = crate::acquire_ecosystem(&installation, &superworkspace.root)
        .map_err(|error| error.to_string())?;

    println!();
    println!("Registration: {}", report.ecosystem_id);
    println!("Registry: {}", report.registry_endpoint);
    println!("Superworkspace: {}", report.superworkspace_root.display());

    println!("Repositories:");

    for repository in &report.repositories {
        println!("  {} -> {}", repository.id, repository.root.display());
    }

    if let Err(error) = synchronize_existing_development_environment(&report.superworkspace_root) {
        eprintln!(
            "warning: source restoration succeeded, but the existing development environment could not be synchronized: {error}"
        );
    }

    Ok(())
}

const LOO_CAST_REPOSITORY: &str = "Loo-Cast";
const VAPOR_CLIENT_REPOSITORY: &str = "Vapor-Client";
const VAPOR_PLATFORM_SERVER_REPOSITORY: &str = "Vapor-Platform-Server";

fn source_selection(selector: &str) -> Result<Vec<&'static str>, String> {
    match selector {
        "loo-cast" => Ok(vec![LOO_CAST_REPOSITORY]),
        "vapor-client" => Ok(vec![VAPOR_CLIENT_REPOSITORY]),
        "vapor-platform-server" => Ok(vec![VAPOR_PLATFORM_SERVER_REPOSITORY]),
        "vapor-all" => Ok(vec![VAPOR_CLIENT_REPOSITORY, VAPOR_PLATFORM_SERVER_REPOSITORY]),
        "first-party-all" => Ok(vec![
            LOO_CAST_REPOSITORY,
            VAPOR_CLIENT_REPOSITORY,
            VAPOR_PLATFORM_SERVER_REPOSITORY,
        ]),
        other => Err(format!(
            "unknown source selector `{other}`; expected loo-cast, vapor-client, vapor-platform-server, vapor-all, or first-party-all"
        )),
    }
}

fn configured_superworkspace(installation: &VaporInstallation) -> Result<VaporSuperworkspace, String> {
    ensure_canonical_superworkspace(installation).map_err(|error| error.to_string())
}

fn source_acquire(selector: &str) -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;
    let superworkspace = configured_superworkspace(&installation)?;
    let selection = source_selection(selector)?;

    let report = acquire_ecosystem_repositories(
        &installation,
        &superworkspace.root,
        &selection,
    )
    .map_err(|error| error.to_string())?;

    println!("Acquired Vapor source:");
    println!("  selector: {selector}");
    println!("  Registry: {}", report.registry_endpoint);
    println!("  Superworkspace: {}", report.superworkspace_root.display());
    for repository in &report.repositories {
        println!("  {} -> {}", repository.id, repository.root.display());
    }

    if let Err(error) = synchronize_existing_development_environment(&report.superworkspace_root) {
        eprintln!("warning: source acquisition succeeded, but development-state synchronization failed: {error}");
    }

    Ok(())
}

fn source_remove(selector: &str, yes: bool) -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;
    let superworkspace = configured_superworkspace(&installation)?;
    let selection = source_selection(selector)?;
    let targets = selection
        .iter()
        .map(|name| ((*name).to_owned(), superworkspace.root.join(name)))
        .filter(|(_, path)| path.exists())
        .collect::<Vec<_>>();

    if targets.is_empty() {
        println!("No selected source is currently acquired.");
        return Ok(());
    }

    // Validate the complete set before deleting any checkout.
    for (name, path) in &targets {
        ensure_source_checkout_safe(name, path)?;
    }

    if !yes && !confirm_source_removal(selector, &targets)? {
        println!("Source removal cancelled.");
        return Ok(());
    }

    for (name, path) in &targets {
        fs::remove_dir_all(path).map_err(|error| {
            format!("failed to remove source `{name}` at `{}`: {error}", path.display())
        })?;
        println!("Removed {name}: {}", path.display());
    }

    Ok(())
}

fn ensure_source_checkout_safe(name: &str, root: &Path) -> Result<(), String> {
    ensure_git_checkout_safe(name, root)?;

    let submodules = git_output(root, &["submodule", "status"])?;
    for line in submodules.lines() {
        let trimmed = line.trim_start_matches(|character| matches!(character, ' ' | '-' | '+' | 'U'));
        let mut fields = trimmed.split_whitespace();
        let _commit = fields.next();
        let Some(path) = fields.next() else {
            continue;
        };
        let submodule = root.join(path);
        if submodule.is_dir() {
            ensure_git_checkout_safe(&format!("{name}/{path}"), &submodule)?;
        }
    }

    Ok(())
}

fn ensure_git_checkout_safe(name: &str, root: &Path) -> Result<(), String> {
    let inside = git_output(root, &["rev-parse", "--is-inside-work-tree"])?;
    if inside != "true" {
        return Err(format!("source `{name}` is not a Git work tree: `{}`", root.display()));
    }

    let dirty = git_output(
        root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    if !dirty.is_empty() {
        return Err(format!(
            "refusing to remove source `{name}` because it has uncommitted/untracked changes:\n{dirty}"
        ));
    }

    git_output(root, &["fetch", "--quiet", "--prune", "origin"])?;

    let local_only = git_output(root, &["rev-list", "--branches", "--not", "--remotes=origin"])?;
    if !local_only.is_empty() {
        return Err(format!(
            "refusing to remove source `{name}` because local branches contain commits not present on origin"
        ));
    }

    let remote_contains_head = git_output(root, &["branch", "-r", "--contains", "HEAD"])?;
    if remote_contains_head.is_empty() {
        return Err(format!(
            "refusing to remove source `{name}` because HEAD is not reachable from an origin branch"
        ));
    }

    let local_tags = git_output(
        root,
        &[
            "for-each-ref",
            "--format=%(refname:strip=2) %(objectname)",
            "refs/tags",
        ],
    )?;
    let remote_tags = git_output(root, &["ls-remote", "--tags", "--refs", "origin"])?;

    let remote_tags = remote_tags
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let object = fields.next()?;
            let reference = fields.next()?;
            Some((
                reference.trim_start_matches("refs/tags/").to_owned(),
                object.to_owned(),
            ))
        })
        .collect::<BTreeMap<_, _>>();

    for line in local_tags.lines() {
        let Some((tag, object)) = line.split_once(' ') else {
            continue;
        };
        if remote_tags.get(tag).map(String::as_str) != Some(object) {
            return Err(format!(
                "refusing to remove source `{name}` because local tag `{tag}` is not identically published on origin"
            ));
        }
    }

    Ok(())
}

fn confirm_source_removal(
    selector: &str,
    targets: &[(String, PathBuf)],
) -> Result<bool, String> {
    eprintln!("WARNING: source removal permanently deletes selected checkouts for `{selector}`.");
    for (_, path) in targets {
        eprintln!("  - {}", path.display());
    }
    eprint!("Type `yes` to continue: ");
    io::stderr()
        .flush()
        .map_err(|error| format!("failed to write confirmation prompt: {error}"))?;

    let mut confirmation = String::new();
    io::stdin()
        .read_line(&mut confirmation)
        .map_err(|error| format!("failed to read confirmation: {error}"))?;

    Ok(confirmation.trim().eq_ignore_ascii_case("yes"))
}

fn source_teardown(yes: bool) -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;
    let superworkspace = configured_superworkspace(&installation)?;

    if !superworkspace.repositories.is_empty() {
        return Err(format!(
            "refusing to tear down Superworkspace `{}` while {} managed repository/repositories remain; remove source first",
            superworkspace.root.display(),
            superworkspace.repositories.len()
        ));
    }

    if !yes {
        eprintln!("WARNING: this removes canonical Superworkspace configuration.");
        eprintln!("  - {}", superworkspace.root.display());
        eprint!("Type `yes` to continue: ");
        io::stderr()
            .flush()
            .map_err(|error| format!("failed to write confirmation prompt: {error}"))?;
        let mut confirmation = String::new();
        io::stdin()
            .read_line(&mut confirmation)
            .map_err(|error| format!("failed to read confirmation: {error}"))?;
        if !confirmation.trim().eq_ignore_ascii_case("yes") {
            println!("Source teardown cancelled.");
            return Ok(());
        }
    }

    let marker = superworkspace.root.join(SUPERWORKSPACE_MANIFEST_FILE_NAME);
    if marker.is_file() {
        fs::remove_file(&marker)
            .map_err(|error| format!("failed to remove `{}`: {error}", marker.display()))?;
    }

    forget_source(&installation, &superworkspace.root).map_err(|error| error.to_string())?;

    match fs::remove_dir(&superworkspace.root) {
        Ok(()) => println!("Removed empty Superworkspace directory: {}", superworkspace.root.display()),
        Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => println!(
            "Superworkspace configuration removed; retained non-empty directory: {}",
            superworkspace.root.display()
        ),
        Err(error) => {
            return Err(format!(
                "Superworkspace configuration was removed, but `{}` could not be cleaned up: {error}",
                superworkspace.root.display()
            ));
        }
    }

    Ok(())
}

fn execute_client(command: ClientCommand) -> Result<(), String> {
    match command {
        ClientCommand::Status => client_status(),

        ClientCommand::Build => run_client_operation(DevelopmentOperation::Build),

        ClientCommand::Test => run_client_operation(DevelopmentOperation::Test),

        ClientCommand::Deploy { command } => execute_client_deploy(command),
    }
}

fn execute_client_deploy(command: ClientDeployCommand) -> Result<(), String> {
    match command {
        ClientDeployCommand::Local => client_deploy_local(),

        ClientDeployCommand::Steam(args) => client_deploy_steam(args),
    }
}

fn execute_platform_server(command: PlatformServerCommand) -> Result<(), String> {
    match command {
        PlatformServerCommand::Status => platform_server_status(),

        PlatformServerCommand::Runs(args) => platform_server_runs(args),

        PlatformServerCommand::Build => platform_server_recipe("build"),

        PlatformServerCommand::Test => platform_server_recipe("test"),

        PlatformServerCommand::Deploy => platform_server_deploy(),
    }
}

fn execute_packagepack(command: PackagepackCommand) -> Result<(), String> {
    match command {
        PackagepackCommand::Create(_) => not_implemented("packagepack", "create"),

        PackagepackCommand::List(args) => content_list(ContentKind::Packagepack, args),

        PackagepackCommand::Inspect(args) => content_inspect(ContentKind::Packagepack, args),

        PackagepackCommand::Resolve(args) => packagepack_resolve(args),

        PackagepackCommand::Verify(_) => not_implemented("packagepack", "verify"),

        PackagepackCommand::Build(args) => packagepack_build(args),

        PackagepackCommand::Test(_) => not_implemented("packagepack", "test"),

        PackagepackCommand::Install(_) => not_implemented("packagepack", "install"),

        PackagepackCommand::Select(_) => not_implemented("packagepack", "select"),

        PackagepackCommand::Run(args) => packagepack_run(args),

        PackagepackCommand::Remove(_) => not_implemented("packagepack", "remove"),

        PackagepackCommand::Publish(_) => not_implemented("packagepack", "publish"),
    }
}

fn execute_graph_content(kind: ContentKind, command: GraphContentCommand) -> Result<(), String> {
    match command {
        GraphContentCommand::Create(_) => not_implemented(kind.as_str(), "create"),

        GraphContentCommand::List(args) => content_list(kind, args),

        GraphContentCommand::Inspect(args) => content_inspect(kind, args),

        GraphContentCommand::Resolve(args) => content_resolve(kind, args),

        GraphContentCommand::Verify(_) => not_implemented(kind.as_str(), "verify"),

        GraphContentCommand::Test(_) => not_implemented(kind.as_str(), "test"),

        GraphContentCommand::Publish(_) => not_implemented(kind.as_str(), "publish"),
    }
}

fn execute_behavioral(kind: ContentKind, command: BehavioralContentCommand) -> Result<(), String> {
    match command {
        BehavioralContentCommand::Create(_) => not_implemented(kind.as_str(), "create"),

        BehavioralContentCommand::List(args) => content_list(kind, args),

        BehavioralContentCommand::Inspect(args) => content_inspect(kind, args),

        BehavioralContentCommand::Verify(_) => not_implemented(kind.as_str(), "verify"),

        BehavioralContentCommand::Test(_) => not_implemented(kind.as_str(), "test"),

        BehavioralContentCommand::Publish(_) => not_implemented(kind.as_str(), "publish"),
    }
}

fn execute_library(command: LibraryCommand) -> Result<(), String> {
    match command {
        LibraryCommand::Create(_) => not_implemented("library", "create"),

        LibraryCommand::List(args) => content_list(ContentKind::Library, args),

        LibraryCommand::Inspect(args) => content_inspect(ContentKind::Library, args),

        LibraryCommand::Resolve(args) => content_resolve(ContentKind::Library, args),

        LibraryCommand::Verify(args) => library_verify(args),

        LibraryCommand::Repair(args) => library_repair(args),

        LibraryCommand::Test(_) => not_implemented("library", "test"),

        LibraryCommand::Publish(_) => not_implemented("library", "publish"),
    }
}

fn installation_diagnose() -> Result<(), String> {
    let status = diagnose_managed_state().map_err(|error| error.to_string())?;

    print_managed_state(&status);

    if status.is_healthy() {
        Ok(())
    } else {
        Err("Vapor managed state requires repair".to_owned())
    }
}

fn installation_repair() -> Result<(), String> {
    let report = repair_managed_state().map_err(|error| error.to_string())?;

    if report.toolchain_installed {
        println!("Installed the pinned Vapor-managed Rust toolchain.");
    }

    if report.development_changes.is_empty() {
        println!("Vapor-managed development state was already current.");
    } else {
        println!("Reconciled Vapor-managed development state:");

        for path in &report.development_changes {
            println!("  {}", path.display());
        }
    }

    println!();
    print_managed_state(&report.status);

    if report.status.is_healthy() {
        Ok(())
    } else {
        Err("Vapor managed state remains unhealthy after safe repair".to_owned())
    }
}

fn print_managed_state(status: &crate::MaintenanceStatus) {
    println!("Vapor managed state:");
    println!("  installation: {}", status.installation_root.display());
    println!("  source: {}", status.source_root.display());
    println!(
        "  toolchain: {} ({})",
        status.toolchain_version,
        if status.toolchain_installed {
            "installed"
        } else {
            "missing"
        },
    );

    match &status.superworkspace_root {
        Some(root) => println!("  Superworkspace: {}", root.display()),
        None => println!("  Superworkspace: unavailable"),
    }

    println!("  Workspaces: {}", status.current_workspaces);
    println!("  Projects: {}", status.current_projects);

    if !status.jetbrains_present {
        println!("  RustRover: not present");
    } else if status
        .ide_status
        .as_ref()
        .is_some_and(crate::IdeStatus::is_current)
    {
        println!("  RustRover: current");
    } else {
        println!("  RustRover: repair required");
    }

    for issue in &status.incompatible_workspaces {
        println!(
            "  incompatible Workspace `{}`: {}",
            issue.name, issue.message
        );
    }
}

fn installation_status() -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let role = role_status(&installation).map_err(|error| error.to_string())?;

    println!("Vapor Installation:");
    println!("  root: {}", installation.root.display());

    println!("  resolved via: {}", installation.root_source);
    println!("  user data: {}", installation.user_data_root().display());
    println!("  state: {}", installation.state_root().display());

    println!("  role: {}", role.installed_role);

    println!("  role state: {}", role.state_path.display());

    Ok(())
}

fn installer_role_status() -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let status = role_status(&installation).map_err(|error| error.to_string())?;

    println!("Vapor Role:");
    println!("  installed: {}", status.installed_role);
    println!("  installation: {}", installation.root.display());
    println!("  state: {}", status.state_path.display());

    println!();
    println!("Capability layers:");
    println!("  Player: ready");

    if status.installed_role >= VaporRole::Composer {
        println!(
            "  Composer: {}",
            if git_available() {
                "ready"
            } else {
                "degraded: Git missing"
            }
        );
    } else {
        println!("  Composer: not installed");
    }

    if status.installed_role >= VaporRole::ContentDeveloper {
        match ManagedToolchain::discover() {
            Ok(toolchain) => {
                println!(
                    "  Content Developer: {}",
                    if toolchain.is_installed() {
                        "ready"
                    } else {
                        "degraded: toolchain missing"
                    }
                );

                println!("    Rust: {}", toolchain.pin.version);
            }

            Err(error) => {
                println!("  Content Developer: degraded: {error}");
            }
        }
    } else {
        println!("  Content Developer: not installed");
    }

    if status.installed_role >= VaporRole::EcosystemDeveloper {
        println!("  Ecosystem Developer: ready");
    } else {
        println!("  Ecosystem Developer: not installed");
    }

    Ok(())
}

fn installer_role_promote(target: VaporRole) -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let current = role_status(&installation)
        .map_err(|error| error.to_string())?
        .installed_role;

    println!("Promoting Vapor Role:");
    println!("  {current} -> {target}");

    let report = promote_role(&installation, target).map_err(|error| error.to_string())?;

    if report.toolchain_installed {
        println!("Installed the Vapor-managed Content Developer toolchain.");
    }

    println!("Installed Role: {}", report.installed_role);

    Ok(())
}

fn installer_role_demote(target: VaporRole) -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let current = role_status(&installation)
        .map_err(|error| error.to_string())?
        .installed_role;

    println!("Demoting Vapor Role:");
    println!("  {current} -> {target}");

    let report = demote_role(&installation, target).map_err(|error| error.to_string())?;

    println!("Installed Role: {}", report.installed_role);
    println!("Existing tooling and authored source were preserved.");

    Ok(())
}

fn authority_status() -> Result<(), String> {
    println!("Vapor Authority:");
    println!("  integration: not implemented");
    println!("  installed Role does not grant protected external authority");

    Ok(())
}

fn toolchain_status() -> Result<(), String> {
    let toolchain = ManagedToolchain::discover().map_err(|error| error.to_string())?;

    println!("Vapor-managed Rust toolchain:");
    println!(
        "  pin: {} {} ({})",
        toolchain.pin.channel, toolchain.pin.version, toolchain.pin.date,
    );
    println!("  installation: {}", toolchain.vapor_home.display());
    println!("  user data: {}", toolchain.user_data_root.display());
    println!("  installation source: {}", toolchain.installation_source);
    println!("  cargo: {}", toolchain.cargo_path.display());
    println!("  rustc: {}", toolchain.rustc_path.display());
    println!(
        "  rust-analyzer: {}",
        toolchain.rust_analyzer_path.display()
    );
    println!(
        "  state: {}",
        if toolchain.is_installed() {
            "installed"
        } else {
            "missing"
        }
    );

    Ok(())
}

fn toolchain_install() -> Result<(), String> {
    let toolchain = ManagedToolchain::discover().map_err(|error| error.to_string())?;

    println!("Installing Vapor-managed Rust {}...", toolchain.pin.version);

    toolchain.install().map_err(|error| error.to_string())?;

    println!(
        "Installed Vapor-managed Rust {} at {}",
        toolchain.pin.version,
        toolchain.user_data_root.display()
    );

    Ok(())
}

const CLIENT_WORKSPACE_REPOSITORY: &str = "Vapor-Client/Vapor";
const CLIENT_WORKSPACE_ORGANIZATION: &str = "ghf-studios";
const CLIENT_WORKSPACE_NAME: &str = "vapor";

const PLATFORM_SERVER_REPOSITORY: &str = "Vapor-Platform-Server";

fn first_party_superworkspace() -> Result<VaporSuperworkspace, String> {
    if let Ok(current_directory) = env::current_dir()
        && let Ok(superworkspace) = VaporSuperworkspace::discover_from(&current_directory)
    {
        return Ok(superworkspace);
    }

    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let source = resolve_source_context(&installation, None).map_err(|error| error.to_string())?;

    VaporSuperworkspace::discover_from(&source.root).map_err(|error| {
        format!(
            "failed to resolve Vapor Superworkspace from source `{}`: {error}",
            source.root.display()
        )
    })
}

fn first_party_repository_root(repository: &str) -> Result<PathBuf, String> {
    let superworkspace = first_party_superworkspace()?;

    superworkspace
        .repositories
        .iter()
        .find(|candidate| candidate.name == repository)
        .map(|candidate| candidate.root.clone())
        .ok_or_else(|| {
            format!(
                "first-party Vapor source `{repository}` is not present in Superworkspace `{}`",
                superworkspace.root.display()
            )
        })
}

fn client_workspace() -> Result<VaporWorkspace, String> {
    // When invoked directly from the Client Workspace, use that explicit local
    // source without requiring previously persisted source context.
    if let Ok(workspace) = VaporWorkspace::discover()
        && workspace.manifest.workspace.organization == CLIENT_WORKSPACE_ORGANIZATION
        && workspace.manifest.workspace.name == CLIENT_WORKSPACE_NAME
    {
        return Ok(workspace);
    }

    let root = first_party_repository_root(CLIENT_WORKSPACE_REPOSITORY)?;

    VaporWorkspace::load(&root).map_err(|error| {
        format!(
            "failed to load first-party Vapor Client Workspace `{}`: {error}",
            root.display()
        )
    })
}

fn client_status() -> Result<(), String> {
    let workspace = client_workspace()?;

    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let toolchain =
        ManagedToolchain::for_workspace(&workspace).map_err(|error| error.to_string())?;

    let sources = source_state(&installation).map_err(|error| error.to_string())?;

    println!("Vapor Client source:");

    println!(
        "  identity: {}/{} {}",
        workspace.manifest.workspace.organization,
        workspace.manifest.workspace.name,
        workspace.manifest.workspace.version
    );

    println!("  root: {}", workspace.root.display());
    println!("  repository: {}", workspace.manifest.workspace.repository);

    println!("  installation: {}", installation.root.display());
    println!("  installation source: {}", installation.root_source);

    if let Ok(executable) = env::current_exe() {
        println!("  command: {}", executable.display());
    }

    println!(
        "  active source: {}",
        sources
            .active
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_owned())
    );

    println!(
        "  toolchain: {} {} ({})",
        toolchain.pin.channel, toolchain.pin.version, toolchain.pin.date
    );

    println!(
        "  toolchain state: {}",
        if toolchain.is_installed() {
            "installed"
        } else {
            "missing"
        }
    );

    println!("  projects:");

    for project in &workspace.projects {
        println!("    {}:", project.name);
        println!("      root: {}", project.root.display());

        println!(
            "      target: {}",
            development_target_dir(&toolchain, project).display()
        );
    }

    Ok(())
}

fn platform_server_status() -> Result<(), String> {
    let superworkspace = first_party_superworkspace()?;

    let platform_server = superworkspace
        .repositories
        .iter()
        .find(|candidate| candidate.name == PLATFORM_SERVER_REPOSITORY)
        .ok_or_else(|| {
            format!(
                "first-party Vapor Platform Server source `{PLATFORM_SERVER_REPOSITORY}` is not present in Superworkspace `{}`",
                superworkspace.root.display()
            )
        })?;

    println!("Vapor Platform Server source:");
    println!("  root: {}", platform_server.root.display());
    println!("  kind: {}", platform_server.kind);

    let prefix = format!("{PLATFORM_SERVER_REPOSITORY}/");

    println!("  Workspaces:");

    let mut found = false;

    for repository in superworkspace
        .repositories
        .iter()
        .filter(|candidate| candidate.name.starts_with(&prefix))
    {
        found = true;

        println!("    {}:", repository.name);
        println!("      root: {}", repository.root.display());
        println!("      kind: {}", repository.kind);
    }

    if !found {
        println!("    none currently active");
    }

    println!();
    print_platform_server_activity_summary();

    Ok(())
}

const PLATFORM_SERVER_GITHUB_REPOSITORY: &str = "GHF-Studios/Vapor-Platform-Server";
const PLATFORM_SERVER_DEPLOY_WORKFLOW: &str = "deploy.yml";

fn platform_server_recipe(recipe: &str) -> Result<(), String> {
    let root = first_party_repository_root(PLATFORM_SERVER_REPOSITORY)?;
    let script = root.join("deploy/scripts").join(format!("{recipe}.sh"));

    if !script.is_file() {
        return Err(format!(
            "Vapor Platform Server does not provide the `{recipe}` recipe at `{}`",
            script.display()
        ));
    }

    let toolchain = ManagedToolchain::discover().map_err(|error| error.to_string())?;
    let mut command = toolchain.command("bash").map_err(|error| error.to_string())?;

    command
        .arg(&script)
        .current_dir(&root)
        .env("CARGO", &toolchain.cargo_path);

    println!(
        "{} Vapor Platform Server with repo-owned `{}`...",
        if recipe == "build" { "Building" } else { "Testing" },
        script.display(),
    );

    let status = command.status().map_err(|error| {
        format!(
            "failed to start Platform Server `{recipe}` recipe `{}`: {error}",
            script.display()
        )
    })?;

    if status.success() {
        println!(
            "{} Vapor Platform Server.",
            if recipe == "build" { "Built" } else { "Tested" }
        );

        Ok(())
    } else {
        Err(format!(
            "Platform Server `{recipe}` recipe exited with {status}"
        ))
    }
}

#[derive(Debug, serde::Deserialize)]
struct GitHubRun {
    #[serde(rename = "databaseId")]
    database_id: u64,
    #[serde(rename = "headSha")]
    head_sha: String,
    status: String,
    conclusion: Option<String>,
    url: String,
}

fn command_output(mut command: Command, description: &str) -> Result<String, String> {
    let output = command
        .output()
        .map_err(|error| format!("failed to start {description}: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "{description} failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_output(root: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args);

    command_output(command, &format!("Git `{}`", args.join(" ")))
}

fn latest_platform_deploy_dispatch() -> Result<Option<GitHubRun>, String> {
    let mut command = Command::new("gh");

    command.args([
        "run",
        "list",
        "--repo",
        PLATFORM_SERVER_GITHUB_REPOSITORY,
        "--workflow",
        PLATFORM_SERVER_DEPLOY_WORKFLOW,
        "--event",
        "workflow_dispatch",
        "--branch",
        "main",
        "--limit",
        "1",
        "--json",
        "databaseId,headSha,status,conclusion,url",
    ]);

    let source = command_output(command, "GitHub CLI deployment-run query")?;
    let runs: Vec<GitHubRun> = serde_json::from_str(&source)
        .map_err(|error| format!("GitHub CLI returned invalid deployment-run JSON: {error}"))?;

    Ok(runs.into_iter().next())
}

fn platform_server_deploy() -> Result<(), String> {
    let root = first_party_repository_root(PLATFORM_SERVER_REPOSITORY)?;

    let dirty = git_output(&root, &["status", "--porcelain=v1"])?;

    if !dirty.is_empty() {
        return Err(format!(
            "Vapor Platform Server source is dirty; deployment requires a committed checkout:\n{dirty}"
        ));
    }

    let branch = git_output(&root, &["branch", "--show-current"])?;

    if branch != "main" {
        return Err(format!(
            "Vapor Platform Server VPS deployment requires local branch `main`; current branch is `{branch}`"
        ));
    }

    let mut fetch = Command::new("git");
    fetch
        .arg("-C")
        .arg(&root)
        .args(["fetch", "--quiet", "origin", "main"]);

    let fetch_status = fetch
        .status()
        .map_err(|error| format!("failed to fetch Platform Server origin/main: {error}"))?;

    if !fetch_status.success() {
        return Err(format!(
            "failed to fetch Platform Server origin/main with {fetch_status}"
        ));
    }

    let head = git_output(&root, &["rev-parse", "HEAD"])?;
    let remote_head = git_output(&root, &["rev-parse", "origin/main"])?;

    if head != remote_head {
        return Err(format!(
            "Vapor Platform Server local HEAD is not the deployed `origin/main` revision.\n  local:  {head}\n  remote: {remote_head}\nCommit/push or update the checkout before deploying."
        ));
    }

    let previous_dispatch = latest_platform_deploy_dispatch()?.map(|run| run.database_id);

    let mut auth = Command::new("gh");
    auth.args(["auth", "status"]);

    let auth_status = auth
        .status()
        .map_err(|error| format!("GitHub CLI `gh` is required for Platform Server deployment: {error}"))?;

    if !auth_status.success() {
        return Err(
            "GitHub CLI is not authenticated; run `gh auth login` before Platform Server deployment"
                .to_owned(),
        );
    }

    println!(
        "Triggering existing Platform Server deployment workflow for {}...",
        &head[..12.min(head.len())],
    );

    let mut trigger = Command::new("gh");
    trigger.args([
        "workflow",
        "run",
        PLATFORM_SERVER_DEPLOY_WORKFLOW,
        "--repo",
        PLATFORM_SERVER_GITHUB_REPOSITORY,
        "--ref",
        "main",
    ]);

    let trigger_status = trigger
        .status()
        .map_err(|error| format!("failed to trigger Platform Server deployment workflow: {error}"))?;

    if !trigger_status.success() {
        return Err(format!(
            "GitHub workflow dispatch failed with {trigger_status}"
        ));
    }

    let run = (0..30)
        .find_map(|_| {
            let result = latest_platform_deploy_dispatch();

            match result {
                Ok(Some(run))
                    if run.head_sha == head && Some(run.database_id) != previous_dispatch =>
                {
                    Some(Ok(run))
                }

                Ok(_) => {
                    thread::sleep(Duration::from_secs(1));
                    None
                }

                Err(error) => Some(Err(error)),
            }
        })
        .transpose()?
        .ok_or_else(|| {
            "GitHub accepted the deployment dispatch, but Vapor could not discover the new workflow run"
                .to_owned()
        })?;

    println!("Deployment workflow: {}", run.url);
    println!(
        "Initial state: {}{}",
        run.status,
        run.conclusion
            .as_deref()
            .map(|value| format!(" ({value})"))
            .unwrap_or_default(),
    );

    let run_id = run.database_id.to_string();
    let mut watch = Command::new("gh");
    watch.args([
        "run",
        "watch",
        &run_id,
        "--repo",
        PLATFORM_SERVER_GITHUB_REPOSITORY,
        "--exit-status",
    ]);

    let watch_status = watch
        .status()
        .map_err(|error| format!("failed to watch Platform Server deployment workflow: {error}"))?;

    if watch_status.success() {
        println!("Vapor Platform Server deployment completed successfully.");
        Ok(())
    } else {
        Err(format!(
            "Platform Server deployment workflow failed with {watch_status}; inspect {}",
            run.url
        ))
    }
}

fn client_deploy_local() -> Result<(), String> {
    let workspace = client_workspace()?;

    // Resolve before self-deployment replaces the running executable.
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    println!(
        "Deploying Vapor Client {}/{} {} locally...",
        workspace.manifest.workspace.organization,
        workspace.manifest.workspace.name,
        workspace.manifest.workspace.version,
    );

    let report = deploy_workspace(&workspace).map_err(|error| error.to_string())?;

    let bootstrap = crate::install_ecosystem_bootstrap(&installation, &workspace.root)
        .map_err(|error| error.to_string())?;

    println!();
    println!("Vapor Installation: {}", report.installation_root.display());

    println!("Deployed binaries:");

    for binary in &report.binaries {
        println!("  {} -> {}", binary.name, binary.destination.display());
    }

    println!("Shell activation: {}", report.activation_script.display());
    println!("Source bootstrap: {}", bootstrap.display());

    if let Err(error) = synchronize_existing_development_environment(&workspace.root) {
        eprintln!(
            "warning: local Client deployment succeeded, but the existing development environment could not be synchronized: {error}"
        );
    }

    Ok(())
}

fn client_deploy_steam(args: SteamDeployArgs) -> Result<(), String> {
    let workspace = client_workspace()?;

    println!(
        "Deploying Vapor Client {}/{} {} to Steam{}...",
        workspace.manifest.workspace.organization,
        workspace.manifest.workspace.name,
        workspace.manifest.workspace.version,
        if args.preview { " (preview)" } else { "" },
    );

    let report = deploy_ecosystem_to_steam(
        &workspace,
        SteamDeploymentOptions {
            preview: args.preview,
            account: args.account,
            steamcmd: args.steamcmd,
        },
    )
    .map_err(|error| error.to_string())?;

    println!();
    println!("Steam deployment:");
    println!("  App ID: {}", report.app_id);
    println!("  branch: {}", report.branch);
    println!("  account: {}", report.account);
    println!("  SteamCMD: {}", report.steamcmd.display());
    println!("  stage: {}", report.stage_root.display());

    println!("  depots:");

    for depot in &report.depots {
        println!(
            "    {} ({}) -> {}",
            depot.name,
            depot.id,
            depot.content_root.display(),
        );
    }

    Ok(())
}

fn run_client_operation(operation: DevelopmentOperation) -> Result<(), String> {
    let workspace = client_workspace()?;

    let verb = match operation {
        DevelopmentOperation::Build => "Building",
        DevelopmentOperation::Test => "Testing",
    };

    println!(
        "{verb} Vapor Client {}/{} {}...",
        workspace.manifest.workspace.organization,
        workspace.manifest.workspace.name,
        workspace.manifest.workspace.version
    );

    run_workspace_operation(&workspace, operation).map_err(|error| error.to_string())?;

    let completed = match operation {
        DevelopmentOperation::Build => "Built",
        DevelopmentOperation::Test => "Tested",
    };

    println!(
        "{completed} Vapor Client {}/{}",
        workspace.manifest.workspace.organization, workspace.manifest.workspace.name
    );

    Ok(())
}

fn source_status() -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let state = source_state(&installation).map_err(|error| error.to_string())?;

    let context = resolve_source_context(&installation, None).ok();

    println!("Vapor source context:");

    println!("  installation: {}", installation.root.display());

    println!("  state: {}", state.state_path.display());

    println!(
        "  Superworkspace: {}",
        state
            .superworkspace
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_owned())
    );

    println!(
        "  active: {}",
        state
            .active
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_owned())
    );

    if let Some(context) = context {
        println!("  effective: {}", context.root.display());
        println!("  effective via: {}", context.source);
    } else {
        println!("  effective: none");
    }

    println!("  known: {}", state.known.len());

    Ok(())
}

fn source_list() -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let state = source_state(&installation).map_err(|error| error.to_string())?;

    println!("{} known Vapor source root(s):", state.known.len());

    for source in &state.known {
        let active = state.active.as_ref() == Some(source);

        println!("{} {}", if active { "*" } else { " " }, source.display());
    }

    Ok(())
}

fn content_list(kind: ContentKind, args: ContentListArgs) -> Result<(), String> {
    let root = source_root(args.root)?;

    let catalog = discover_local_content(&root).map_err(|error| error.to_string())?;

    let matches: Vec<_> = catalog
        .iter()
        .filter(|content| content.manifest.content.kind == kind)
        .collect();

    println!("{} {} artifact(s):", matches.len(), kind);

    for content in matches {
        println!("{}  {}", content.version_id(), content.root.display());
    }

    Ok(())
}

fn content_inspect(kind: ContentKind, args: LocalContentTargetArgs) -> Result<(), String> {
    let root = source_root(args.root)?;

    let catalog = discover_local_content(&root).map_err(|error| error.to_string())?;

    let content = select_content(&catalog, kind, args.id.as_ref())?;

    println!("{}:", content.version_id());
    println!("  kind: {}", content.manifest.content.kind);
    println!("  root: {}", content.root.display());
    println!("  manifest: {}", content.manifest_path.display());

    println!("  dependencies:");

    if content.manifest.dependencies.is_empty() {
        println!("    none");
    } else {
        for (binding, dependency) in &content.manifest.dependencies {
            println!("    {binding}: {} {}", dependency.id, dependency.version);
        }
    }

    Ok(())
}

fn packagepack_resolve(args: LocalContentTargetArgs) -> Result<(), String> {
    let (_, _, composition) = prepare_packagepack(args)?;

    print_composition(&composition);

    Ok(())
}

fn content_resolve(kind: ContentKind, args: LocalContentTargetArgs) -> Result<(), String> {
    let (_, catalog, graph) = prepare_content_graph(kind, args)?;

    print_resolved_graph(&graph);

    match kind {
        ContentKind::Enginepack => {
            let engine = graph
                .content_of_kind(ContentKind::Engine)
                .next()
                .expect("validated Enginepack has one Engine");

            println!();
            println!("Effective Engine: {}", engine.identity);
        }

        ContentKind::Gamepack => {
            let game = graph
                .content_of_kind(ContentKind::Game)
                .next()
                .expect("validated Gamepack has one Game");

            println!();
            println!("Effective Game: {}", game.identity);
        }

        ContentKind::Modpack => {
            let count = graph
                .nodes
                .values()
                .filter(|node| {
                    matches!(
                        node.kind,
                        ContentKind::EngineMod | ContentKind::GameMod | ContentKind::ExtensionMod
                    )
                })
                .count();

            println!();
            println!("Effective Mods: {count}");
        }

        ContentKind::Library => {
            let content = catalog
                .get(&graph.root)
                .expect("resolved local Library must remain in the local catalog");

            let package =
                inspect_local_cargo_package(content).map_err(|error| error.to_string())?;

            if !package.has_library_target() {
                return Err(format!(
                    "Vapor Library `{}` maps to Cargo package `{} {}`, \
                     but that package exposes no Rust library target",
                    graph.root, package.name, package.version
                ));
            }

            print_cargo_package(&package);
        }

        _ => {}
    }

    Ok(())
}

fn library_verify(args: LocalContentTargetArgs) -> Result<(), String> {
    let (_, catalog, graph) = prepare_content_graph(ContentKind::Library, args)?;

    let reconciliation = verify_local_library_cargo_dependencies(&catalog, &graph)
        .map_err(|error| error.to_string())?;

    print_library_cargo_reconciliation(&reconciliation);

    if reconciliation.is_valid() {
        Ok(())
    } else {
        Err(format!(
            "Cargo realization for `{}` does not match its resolved Vapor dependencies",
            reconciliation.library
        ))
    }
}

fn library_repair(args: LocalContentTargetArgs) -> Result<(), String> {
    let (_, catalog, graph) = prepare_content_graph(ContentKind::Library, args)?;

    let report = repair_local_library_cargo_dependencies(&catalog, &graph)
        .map_err(|error| error.to_string())?;

    if report.added_bindings.is_empty() {
        println!("Cargo realization already required no repair.");
    } else {
        println!(
            "Added Cargo dependency binding(s): {}",
            report.added_bindings.join(", ")
        );
    }

    println!();
    print_library_cargo_reconciliation(&report.reconciliation);

    Ok(())
}

fn print_library_cargo_reconciliation(reconciliation: &LibraryCargoReconciliation) {
    println!("Cargo realization for {}:", reconciliation.library);
    println!(
        "  package: {} {}",
        reconciliation.package.name, reconciliation.package.version
    );
    println!(
        "  manifest: {}",
        reconciliation.package.manifest_path.display()
    );
    println!("  dependencies:");

    if reconciliation.dependencies.is_empty() {
        println!("    none");
        return;
    }

    for dependency in &reconciliation.dependencies {
        println!(
            "    {} -> {}: {}",
            dependency.binding, dependency.dependency, dependency.state
        );

        match &dependency.state {
            CargoDependencyState::Conflict { declarations } => {
                for declaration in declarations {
                    println!("      Cargo: {declaration}");
                }
            }

            CargoDependencyState::Unresolved { message } => {
                println!("      {message}");
            }

            _ => {}
        }
    }
}

fn print_cargo_package(package: &CargoPackageInspection) {
    println!();
    println!("Physical Cargo package:");
    println!("  name: {}", package.name);
    println!("  version: {}", package.version);
    println!("  manifest: {}", package.manifest_path.display());
    println!("  workspace: {}", package.workspace_root.display());
    println!("  library targets:");

    for target in package.library_targets() {
        println!(
            "    {} [{}] {}",
            target.name,
            target.crate_types.join(", "),
            target.src_path.display()
        );
    }
}

fn packagepack_build(args: LocalContentTargetArgs) -> Result<(), String> {
    let (root, catalog, composition) = prepare_packagepack(args)?;

    let realization = generate_local_cargo_realization(&root, &catalog, &composition)
        .map_err(|error| error.to_string())?;

    println!(
        "Generated Cargo realization at {}",
        realization.root.display()
    );

    build_cargo_realization(&realization).map_err(|error| error.to_string())?;

    println!("Built {}", composition.root);

    Ok(())
}

fn packagepack_run(args: LocalContentTargetArgs) -> Result<(), String> {
    let (root, catalog, composition) = prepare_packagepack(args)?;

    let realization = generate_local_cargo_realization(&root, &catalog, &composition)
        .map_err(|error| error.to_string())?;

    run_cargo_realization(&realization).map_err(|error| error.to_string())
}

fn prepare_content_graph(
    kind: ContentKind,
    args: LocalContentTargetArgs,
) -> Result<(PathBuf, LocalCatalog, ResolvedContentGraph), String> {
    let root = source_root(args.root)?;

    let catalog = discover_local_content(&root).map_err(|error| error.to_string())?;

    let content = select_content(&catalog, kind, args.id.as_ref())?;

    let id = content.manifest.content.id.clone();

    let graph =
        resolve_local_content_kind(&catalog, &id, kind).map_err(|error| error.to_string())?;

    Ok((root, catalog, graph))
}

fn prepare_packagepack(
    args: LocalContentTargetArgs,
) -> Result<(PathBuf, LocalCatalog, ResolvedComposition), String> {
    let root = source_root(args.root)?;

    let catalog = discover_local_content(&root).map_err(|error| error.to_string())?;

    let content = select_content(&catalog, ContentKind::Packagepack, args.id.as_ref())?;

    let packagepack_id = content.manifest.content.id.clone();

    let composition =
        resolve_local_packagepack(&catalog, &packagepack_id).map_err(|error| error.to_string())?;

    Ok((root, catalog, composition))
}

fn select_content<'a>(
    catalog: &'a LocalCatalog,
    kind: ContentKind,
    id: Option<&'a VaporId>,
) -> Result<&'a LocalContent, String> {
    if let Some(id) = id {
        let content = catalog
            .latest(id)
            .ok_or_else(|| format!("no local Vapor Content is available for `{id}`"))?;

        if content.manifest.content.kind != kind {
            return Err(format!(
                "`{}` is {}, not {}",
                content.version_id(),
                content.manifest.content.kind,
                kind
            ));
        }

        return Ok(content);
    }

    let matching: Vec<_> = catalog
        .iter()
        .filter(|content| content.manifest.content.kind == kind)
        .collect();

    let Some(first) = matching.first() else {
        return Err(format!(
            "no local {kind} is available in this source context"
        ));
    };

    let first_id = &first.manifest.content.id;

    if matching
        .iter()
        .any(|content| &content.manifest.content.id != first_id)
    {
        return Err(format!(
            "multiple local {kind} identities are available; specify a Vapor ID"
        ));
    }

    catalog
        .latest(first_id)
        .ok_or_else(|| format!("failed to select local {kind}"))
}

fn source_root(root: Option<PathBuf>) -> Result<PathBuf, String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    resolve_source_context(&installation, root)
        .map(|context| context.root)
        .map_err(|error| error.to_string())
}

fn print_resolved_graph(graph: &ResolvedContentGraph) {
    println!("Resolved {}:", graph.root);

    print_graph_dependencies(graph, &graph.root, "");
}

fn print_graph_dependencies(
    graph: &ResolvedContentGraph,
    identity: &ContentVersionId,
    prefix: &str,
) {
    let Some(node) = graph.node(identity) else {
        return;
    };

    let dependency_count = node.dependencies.len();

    for (index, (binding, dependency)) in node.dependencies.iter().enumerate() {
        let last = index + 1 == dependency_count;
        let connector = if last { "└── " } else { "├── " };

        println!("{prefix}{connector}{binding} -> {dependency}");

        let child_prefix = format!("{prefix}{}", if last { "    " } else { "│   " });

        print_graph_dependencies(graph, dependency, &child_prefix);
    }
}

fn print_composition(composition: &ResolvedComposition) {
    println!("Resolved {}:", composition.root);

    print_dependencies(composition, &composition.root, "");

    println!();
    println!("Effective Engine: {}", composition.effective_engine);
    println!("Effective Game:   {}", composition.effective_game);
}

fn print_dependencies(
    composition: &ResolvedComposition,
    identity: &ContentVersionId,
    prefix: &str,
) {
    let Some(node) = composition.node(identity) else {
        return;
    };

    let dependency_count = node.dependencies.len();

    for (index, (binding, dependency)) in node.dependencies.iter().enumerate() {
        let last = index + 1 == dependency_count;

        let connector = if last { "└── " } else { "├── " };

        println!("{prefix}{connector}{binding} -> {dependency}");

        let child_prefix = format!("{prefix}{}", if last { "    " } else { "│   " });

        print_dependencies(composition, dependency, &child_prefix);
    }
}

fn not_implemented(domain: &str, operation: &str) -> Result<(), String> {
    Err(format!(
        "`vapor {domain} {operation}` is part of the CLI model but is not implemented yet"
    ))
}
