//! Command-line projections over Vapor Core.
//!
//! Executables are intentionally thin. The universal Vapor CLI and dedicated
//! application CLIs expose subsets of the same underlying core operations.

mod commands;

use crate::{
    ContentKind, ContentVersionId, DevelopmentOperation, LocalCatalog, LocalContent,
    ManagedToolchain, ResolvedComposition, ResolvedContentGraph, VaporId, VaporInstallation,
    VaporRole, VaporWorkspace, build_cargo_realization, demote_role, development_target_dir,
    discover_local_content, generate_local_cargo_realization, git_available, promote_role,
    resolve_local_pack, resolve_local_packagepack, role_status, run_cargo_realization,
    run_workspace_operation,
};
use clap::Parser;
use commands::*;
use std::env;
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

        VaporCommand::Enginepack { command } => execute_pack(ContentKind::Enginepack, command),

        VaporCommand::Gamepack { command } => execute_pack(ContentKind::Gamepack, command),

        VaporCommand::Modpack { command } => execute_pack(ContentKind::Modpack, command),

        VaporCommand::Engine { command } => execute_behavioral(ContentKind::Engine, command),

        VaporCommand::Game { command } => execute_behavioral(ContentKind::Game, command),

        VaporCommand::EngineMod { command } => execute_behavioral(ContentKind::EngineMod, command),

        VaporCommand::GameMod { command } => execute_behavioral(ContentKind::GameMod, command),

        VaporCommand::ExtensionMod { command } => {
            execute_behavioral(ContentKind::ExtensionMod, command)
        }
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
    }
}

fn execute_source(command: SourceCommand) -> Result<(), String> {
    match command {
        SourceCommand::Status => not_implemented("source", "status"),

        SourceCommand::List => not_implemented("source", "list"),

        SourceCommand::Acquire { .. } => not_implemented("source", "acquire"),

        SourceCommand::Fork { .. } => not_implemented("source", "fork"),
    }
}

fn execute_ecosystem(command: EcosystemCommand) -> Result<(), String> {
    match command {
        EcosystemCommand::Status => ecosystem_status(),

        EcosystemCommand::Build => run_ecosystem_operation(DevelopmentOperation::Build),

        EcosystemCommand::Test => run_ecosystem_operation(DevelopmentOperation::Test),

        EcosystemCommand::Acquire { .. } => not_implemented("ecosystem", "acquire"),

        EcosystemCommand::Fork { .. } => not_implemented("ecosystem", "fork"),

        EcosystemCommand::Create { .. } => not_implemented("ecosystem", "create"),

        EcosystemCommand::Publish => not_implemented("ecosystem", "publish"),

        EcosystemCommand::Deploy => not_implemented("ecosystem", "deploy"),
    }
}

fn execute_packagepack(command: PackagepackCommand) -> Result<(), String> {
    match command {
        PackagepackCommand::Create(_) => not_implemented("packagepack", "create"),

        PackagepackCommand::List(args) => content_list(ContentKind::Packagepack, args),

        PackagepackCommand::Inspect(args) => content_inspect(ContentKind::Packagepack, args),

        PackagepackCommand::Resolve(args) => packagepack_resolve(args),

        PackagepackCommand::Build(args) => packagepack_build(args),

        PackagepackCommand::Run(args) => packagepack_run(args),

        PackagepackCommand::Verify(_) => not_implemented("packagepack", "verify"),

        PackagepackCommand::Test(_) => not_implemented("packagepack", "test"),

        PackagepackCommand::Install(_) => not_implemented("packagepack", "install"),

        PackagepackCommand::Select(_) => not_implemented("packagepack", "select"),

        PackagepackCommand::Remove(_) => not_implemented("packagepack", "remove"),

        PackagepackCommand::Publish(_) => not_implemented("packagepack", "publish"),
    }
}

fn execute_pack(kind: ContentKind, command: PackCommand) -> Result<(), String> {
    match command {
        PackCommand::Create(_) => not_implemented(kind.as_str(), "create"),

        PackCommand::List(args) => content_list(kind, args),

        PackCommand::Inspect(args) => content_inspect(kind, args),

        PackCommand::Resolve(args) => pack_resolve(kind, args),

        PackCommand::Verify(_) => not_implemented(kind.as_str(), "verify"),

        PackCommand::Test(_) => not_implemented(kind.as_str(), "test"),

        PackCommand::Publish(_) => not_implemented(kind.as_str(), "publish"),
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

fn installation_status() -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let role = role_status(&installation).map_err(|error| error.to_string())?;

    println!("Vapor Installation:");
    println!("  root: {}", installation.root.display());
    println!("  source: {}", installation.root_source);
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

    let toolchain =
        ManagedToolchain::for_workspace(&workspace).map_err(|error| error.to_string())?;

    println!("Vapor ecosystem source:");
    println!(
        "  identity: {}/{} {}",
        workspace.manifest.workspace.organization,
        workspace.manifest.workspace.name,
        workspace.manifest.workspace.version
    );
    println!("  root: {}", workspace.root.display());
    println!("  repository: {}", workspace.manifest.workspace.repository);
    println!("  installation: {}", toolchain.vapor_home.display());
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

fn pack_resolve(kind: ContentKind, args: LocalContentTargetArgs) -> Result<(), String> {
    let (_, _, graph) = prepare_pack(kind, args)?;

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

        _ => {}
    }

    Ok(())
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

fn prepare_pack(
    kind: ContentKind,
    args: LocalContentTargetArgs,
) -> Result<(PathBuf, LocalCatalog, ResolvedContentGraph), String> {
    let root = source_root(args.root)?;

    let catalog = discover_local_content(&root).map_err(|error| error.to_string())?;

    let content = select_content(&catalog, kind, args.id.as_ref())?;

    let id = content.manifest.content.id.clone();

    let graph = resolve_local_pack(&catalog, &id, kind).map_err(|error| error.to_string())?;

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
    match root {
        Some(root) => Ok(root),

        None => env::current_dir()
            .map_err(|error| format!("failed to determine current source context: {error}")),
    }
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
