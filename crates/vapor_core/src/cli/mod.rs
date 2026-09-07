//! Command-line projections over Vapor Core.
//!
//! Executables are intentionally thin. The universal Vapor CLI and dedicated
//! application CLIs expose subsets of the same underlying core operations.

mod commands;

use crate::{
    CargoDependencyState, CargoPackageInspection, ContentKind, ContentVersionId,
    DevelopmentOperation, LibraryCargoReconciliation, LocalCatalog, LocalContent, ManagedToolchain,
    ResolvedComposition, ResolvedContentGraph, SteamDeploymentOptions, VaporId, VaporInstallation,
    VaporProject, VaporRole, VaporSuperworkspace, VaporWorkspace, build_cargo_realization,
    demote_role, deploy_ecosystem_to_steam, deploy_workspace, development_target_dir,
    discover_local_content, generate_local_cargo_realization, git_available,
    inspect_local_cargo_package, promote_role, reconcile_existing_development_environment,
    repair_local_library_cargo_dependencies, resolve_local_content_kind, resolve_local_packagepack,
    resolve_source_context, role_status, run_cargo_realization, run_workspace_operation,
    source_state, verify_local_library_cargo_dependencies,
};
use clap::Parser;
use commands::*;
use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

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

        VaporCommand::Ecosystem { command } => execute_ecosystem(command),

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
        InstallerCommand::Installation { command } => execute_installation(command),

        InstallerCommand::Role { command } => execute_role(command),

        InstallerCommand::Authority { command } => execute_authority(command),

        InstallerCommand::Toolchain { command } => execute_toolchain(command),
    }
}

fn execute_installation(command: InstallationCommand) -> Result<(), String> {
    match command {
        InstallationCommand::Status => installation_status(),

        InstallationCommand::Diagnose => not_implemented("installation", "diagnose"),

        InstallationCommand::Repair => not_implemented("installation", "repair"),
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

    let needs_project = explicit_project.is_some()
        || !package_hints.is_empty()
        || cargo_needs_project_context(args);

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

        SourceCommand::Acquire { .. } => not_implemented("source", "acquire"),

        SourceCommand::Fork { .. } => not_implemented("source", "fork"),
    }
}

fn synchronize_existing_development_environment(source_root: &Path) -> Result<(), String> {
    let changed = reconcile_existing_development_environment(source_root)
        .map_err(|error| error.to_string())?;

    if !changed.is_empty() {
        println!("Synchronized existing Vapor development environment.");
    }

    Ok(())
}

fn execute_ecosystem(command: EcosystemCommand) -> Result<(), String> {
    match command {
        EcosystemCommand::Status => ecosystem_status(),

        EcosystemCommand::Acquire { source } => ecosystem_acquire(source),

        EcosystemCommand::Fork { .. } => not_implemented("ecosystem", "fork"),

        EcosystemCommand::Create { .. } => not_implemented("ecosystem", "create"),

        EcosystemCommand::Build => run_ecosystem_operation(DevelopmentOperation::Build),

        EcosystemCommand::Test => run_ecosystem_operation(DevelopmentOperation::Test),

        EcosystemCommand::Publish => not_implemented("ecosystem", "publish"),

        EcosystemCommand::Deploy { command } => execute_ecosystem_deploy(command),
    }
}

fn execute_ecosystem_deploy(command: EcosystemDeployCommand) -> Result<(), String> {
    match command {
        EcosystemDeployCommand::Local => ecosystem_deploy_local(),

        EcosystemDeployCommand::Steam(args) => ecosystem_deploy_steam(args),
    }
}

fn ecosystem_acquire(source: Option<String>) -> Result<(), String> {
    if let Some(source) = source {
        return Err(format!(
            "explicit ecosystem source `{source}` is modeled but not implemented yet"
        ));
    }

    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let destination = env::current_dir()
        .map_err(|error| format!("failed to determine acquisition destination: {error}"))?;

    println!("Acquiring Vapor ecosystem from Registry...");

    let report =
        crate::acquire_ecosystem(&installation, &destination).map_err(|error| error.to_string())?;

    println!();
    println!("Ecosystem: {}", report.ecosystem_id);
    println!("Registry: {}", report.registry_endpoint);
    println!("Superworkspace: {}", report.superworkspace_root.display());

    println!("Repositories:");

    for repository in &report.repositories {
        println!("  {} -> {}", repository.id, repository.root.display());
    }

    if let Err(error) = synchronize_existing_development_environment(&report.superworkspace_root) {
        eprintln!(
            "warning: acquisition succeeded, but the existing development environment could not be synchronized: {error}"
        );
    }

    Ok(())
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

fn installation_status() -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let role = role_status(&installation).map_err(|error| error.to_string())?;

    println!("Vapor Installation:");
    println!("  root: {}", installation.root.display());

    println!("  resolved via: {}", installation.root_source);

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
        toolchain.vapor_home.display()
    );

    Ok(())
}

fn ecosystem_status() -> Result<(), String> {
    let workspace = VaporWorkspace::discover().map_err(|error| error.to_string())?;

    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let toolchain =
        ManagedToolchain::for_workspace(&workspace).map_err(|error| error.to_string())?;

    let sources = source_state(&installation).map_err(|error| error.to_string())?;

    println!("Vapor ecosystem source:");
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

fn ecosystem_deploy_local() -> Result<(), String> {
    let workspace = VaporWorkspace::discover().map_err(|error| error.to_string())?;

    // Resolve the active Installation before deployment replaces the
    // currently-running Vapor executable.
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    println!(
        "Deploying Vapor ecosystem {}/{} {} locally...",
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
    println!("Ecosystem bootstrap: {}", bootstrap.display());

    if let Err(error) = synchronize_existing_development_environment(&workspace.root) {
        eprintln!(
            "warning: local ecosystem deployment succeeded, but the existing development environment could not be synchronized: {error}"
        );
    }

    Ok(())
}

fn ecosystem_deploy_steam(args: SteamDeployArgs) -> Result<(), String> {
    let workspace = VaporWorkspace::discover().map_err(|error| error.to_string())?;

    println!(
        "Deploying Vapor ecosystem {}/{} {} to Steam{}...",
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

fn run_ecosystem_operation(operation: DevelopmentOperation) -> Result<(), String> {
    let workspace = VaporWorkspace::discover().map_err(|error| error.to_string())?;

    let verb = match operation {
        DevelopmentOperation::Build => "Building",
        DevelopmentOperation::Test => "Testing",
    };

    println!(
        "{verb} Vapor ecosystem source {}/{} {}...",
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
        "{completed} Vapor ecosystem source {}/{}",
        workspace.manifest.workspace.organization, workspace.manifest.workspace.name
    );

    Ok(())
}

fn source_status() -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let state = source_state(&installation).map_err(|error| error.to_string())?;

    let context = resolve_source_context(&installation, None).map_err(|error| error.to_string())?;

    println!("Vapor source context:");

    println!("  installation: {}", installation.root.display());

    println!("  state: {}", state.state_path.display());

    println!(
        "  active: {}",
        state
            .active
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "none".to_owned())
    );

    println!("  effective: {}", context.root.display());

    println!("  effective via: {}", context.source);

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
