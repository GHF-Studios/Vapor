//! Command-line projections over Vapor Core.
//!
//! Executables are intentionally thin. The umbrella Vapor CLI and dedicated
//! application CLIs expose different subsets of the same underlying core
//! operations.

use crate::{
    ContentVersionId, DevelopmentOperation, LocalCatalog, ManagedToolchain, ResolvedComposition,
    VaporId, VaporInstallation, VaporRole, VaporWorkspace, build_cargo_realization, demote_role,
    development_target_dir, discover_local_content, generate_local_cargo_realization,
    git_available, promote_role, resolve_local_packagepack, role_status, run_cargo_realization,
    run_workspace_operation,
};
use std::env;
use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliSurface {
    Vapor,
    Installer,
}

pub fn run_cli(surface: CliSurface) -> ExitCode {
    match run(surface) {
        Ok(()) => ExitCode::SUCCESS,

        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(surface: CliSurface) -> Result<(), String> {
    let mut arguments = env::args_os();

    let _executable = arguments.next();

    match surface {
        CliSurface::Vapor => run_vapor(&mut arguments),
        CliSurface::Installer => run_installer(&mut arguments),
    }
}

fn run_vapor(arguments: &mut impl Iterator<Item = OsString>) -> Result<(), String> {
    let Some(command) = arguments.next() else {
        return Err(vapor_usage());
    };

    match command.to_str() {
        Some("installer") => run_installer(arguments),

        Some("toolchain") => run_toolchain(arguments),

        Some("workspace") => run_workspace(arguments),

        Some("discover") => {
            let root = required_argument(arguments, "source root", &vapor_usage)?;

            reject_extra_arguments(arguments, &vapor_usage)?;

            discover(Path::new(&root))
        }

        Some("resolve") => {
            let root = required_argument(arguments, "source root", &vapor_usage)?;

            let packagepack_id =
                required_vapor_id(arguments, "Packagepack Vapor ID", &vapor_usage)?;

            reject_extra_arguments(arguments, &vapor_usage)?;

            resolve(Path::new(&root), &packagepack_id)
        }

        Some("build") => {
            let root = required_argument(arguments, "source root", &vapor_usage)?;

            let packagepack_id =
                required_vapor_id(arguments, "Packagepack Vapor ID", &vapor_usage)?;

            reject_extra_arguments(arguments, &vapor_usage)?;

            build(Path::new(&root), &packagepack_id)
        }

        Some("run") => {
            let root = required_argument(arguments, "source root", &vapor_usage)?;

            let packagepack_id =
                required_vapor_id(arguments, "Packagepack Vapor ID", &vapor_usage)?;

            reject_extra_arguments(arguments, &vapor_usage)?;

            run_packagepack(Path::new(&root), &packagepack_id)
        }

        Some(command) => Err(format!(
            "unknown Vapor command `{command}`\n\n{}",
            vapor_usage()
        )),

        None => Err("Vapor command is not valid UTF-8".to_owned()),
    }
}

fn run_installer(arguments: &mut impl Iterator<Item = OsString>) -> Result<(), String> {
    let Some(command) = arguments.next() else {
        return Err(installer_usage());
    };

    match command.to_str() {
        Some("role") => run_installer_role(arguments),

        Some(command) => Err(format!(
            "unknown Installer command `{command}`\n\n{}",
            installer_usage()
        )),

        None => Err("Installer command is not valid UTF-8".to_owned()),
    }
}

fn run_installer_role(arguments: &mut impl Iterator<Item = OsString>) -> Result<(), String> {
    let action = required_argument(arguments, "role action", &installer_usage)?;

    match action.to_str() {
        Some("status") => {
            reject_extra_arguments(arguments, &installer_usage)?;
            installer_role_status()
        }

        Some("promote") => {
            let role = required_role(arguments, "target role", &installer_usage)?;

            reject_extra_arguments(arguments, &installer_usage)?;

            installer_role_promote(role)
        }

        Some("demote") => {
            let role = required_role(arguments, "target role", &installer_usage)?;

            reject_extra_arguments(arguments, &installer_usage)?;

            installer_role_demote(role)
        }

        Some(action) => Err(format!(
            "unknown Installer role action `{action}`\n\n{}",
            installer_usage()
        )),

        None => Err("Installer role action is not valid UTF-8".to_owned()),
    }
}

fn installer_role_status() -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let status = role_status(&installation).map_err(|error| error.to_string())?;

    println!("Vapor Installer role:");
    println!("  installed: {}", status.installed_role);
    println!("  installation: {}", installation.root.display());
    println!("  installation source: {}", installation.root_source);
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

                println!(
                    "    toolchain root: {}",
                    toolchain
                        .cargo_path
                        .parent()
                        .and_then(Path::parent)
                        .map_or_else(
                            || toolchain.vapor_home.display().to_string(),
                            |path| path.display().to_string(),
                        )
                );
            }

            Err(error) => {
                println!("  Content Developer: degraded: {error}");
            }
        }
    } else {
        println!("  Content Developer: not installed");
    }

    if status.installed_role >= VaporRole::EcosystemDeveloper {
        println!("  Ecosystem Developer: installed");
    } else {
        println!("  Ecosystem Developer: not installed");
    }

    if status.installed_role >= VaporRole::RootAuthority {
        println!("  Root Authority: installed");
    } else {
        println!("  Root Authority: not installed");
    }

    Ok(())
}

fn installer_role_promote(target: VaporRole) -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let current = role_status(&installation)
        .map_err(|error| error.to_string())?
        .installed_role;

    println!("Promoting Vapor role:");
    println!("  {current} -> {target}");

    let report = promote_role(&installation, target).map_err(|error| error.to_string())?;

    if report.toolchain_installed {
        println!("Installed the Vapor-managed Content Developer toolchain.");
    }

    println!("Installed role: {}", report.installed_role);

    Ok(())
}

fn installer_role_demote(target: VaporRole) -> Result<(), String> {
    let installation = VaporInstallation::discover().map_err(|error| error.to_string())?;

    let current = role_status(&installation)
        .map_err(|error| error.to_string())?
        .installed_role;

    println!("Demoting Vapor role:");
    println!("  {current} -> {target}");

    let report = demote_role(&installation, target).map_err(|error| error.to_string())?;

    println!("Installed role: {}", report.installed_role);

    println!("Existing tooling and authored source were preserved.");

    Ok(())
}

fn run_toolchain(arguments: &mut impl Iterator<Item = OsString>) -> Result<(), String> {
    let action = required_argument(arguments, "toolchain action", &vapor_usage)?;

    reject_extra_arguments(arguments, &vapor_usage)?;

    match action.to_str() {
        Some("status") => toolchain_status(),
        Some("install") => toolchain_install(),

        Some(action) => Err(format!(
            "unknown toolchain action `{action}`\n\n{}",
            vapor_usage()
        )),

        None => Err("toolchain action is not valid UTF-8".to_owned()),
    }
}

fn run_workspace(arguments: &mut impl Iterator<Item = OsString>) -> Result<(), String> {
    let action = required_argument(arguments, "workspace action", &vapor_usage)?;

    reject_extra_arguments(arguments, &vapor_usage)?;

    match action.to_str() {
        Some("status") => workspace_status(),
        Some("build") => workspace_build(),
        Some("test") => workspace_test(),

        Some(action) => Err(format!(
            "unknown workspace action `{action}`\n\n{}",
            vapor_usage()
        )),

        None => Err("workspace action is not valid UTF-8".to_owned()),
    }
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

fn workspace_status() -> Result<(), String> {
    let workspace = VaporWorkspace::discover().map_err(|error| error.to_string())?;

    let toolchain =
        ManagedToolchain::for_workspace(&workspace).map_err(|error| error.to_string())?;

    println!("Vapor Workspace:");
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

fn workspace_build() -> Result<(), String> {
    run_development_operation(DevelopmentOperation::Build)
}

fn workspace_test() -> Result<(), String> {
    run_development_operation(DevelopmentOperation::Test)
}

fn run_development_operation(operation: DevelopmentOperation) -> Result<(), String> {
    let workspace = VaporWorkspace::discover().map_err(|error| error.to_string())?;

    let verb = match operation {
        DevelopmentOperation::Build => "Building",
        DevelopmentOperation::Test => "Testing",
    };

    println!(
        "{verb} Vapor Workspace {}/{} {}...",
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
        "{completed} Vapor Workspace {}/{}",
        workspace.manifest.workspace.organization, workspace.manifest.workspace.name
    );

    Ok(())
}

fn discover(root: &Path) -> Result<(), String> {
    let catalog = discover_local_content(root).map_err(|error| error.to_string())?;

    println!("Discovered {} Vapor Content artifact(s):", catalog.len());

    for content in catalog.iter() {
        println!(
            "{}  {}  {}",
            content.version_id(),
            content.manifest.content.kind,
            content.root.display()
        );
    }

    Ok(())
}

fn resolve(root: &Path, packagepack_id: &VaporId) -> Result<(), String> {
    let (_, composition) = prepare_composition(root, packagepack_id)?;

    print_composition(&composition);

    Ok(())
}

fn build(root: &Path, packagepack_id: &VaporId) -> Result<(), String> {
    let (catalog, composition) = prepare_composition(root, packagepack_id)?;

    let realization = generate_local_cargo_realization(root, &catalog, &composition)
        .map_err(|error| error.to_string())?;

    println!(
        "Generated Cargo realization at {}",
        realization.root.display()
    );

    build_cargo_realization(&realization).map_err(|error| error.to_string())?;

    println!("Built {}", composition.root);

    Ok(())
}

fn run_packagepack(root: &Path, packagepack_id: &VaporId) -> Result<(), String> {
    let (catalog, composition) = prepare_composition(root, packagepack_id)?;

    let realization = generate_local_cargo_realization(root, &catalog, &composition)
        .map_err(|error| error.to_string())?;

    run_cargo_realization(&realization).map_err(|error| error.to_string())
}

fn prepare_composition(
    root: &Path,
    packagepack_id: &VaporId,
) -> Result<(LocalCatalog, ResolvedComposition), String> {
    let catalog = discover_local_content(root).map_err(|error| error.to_string())?;

    let composition =
        resolve_local_packagepack(&catalog, packagepack_id).map_err(|error| error.to_string())?;

    Ok((catalog, composition))
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

fn required_argument(
    arguments: &mut impl Iterator<Item = OsString>,
    name: &str,
    usage: &dyn Fn() -> String,
) -> Result<OsString, String> {
    arguments
        .next()
        .ok_or_else(|| format!("missing {name}\n\n{}", usage()))
}

fn required_vapor_id(
    arguments: &mut impl Iterator<Item = OsString>,
    name: &str,
    usage: &dyn Fn() -> String,
) -> Result<VaporId, String> {
    let value = required_argument(arguments, name, usage)?;

    let value = value
        .to_str()
        .ok_or_else(|| format!("{name} is not valid UTF-8"))?;

    value.parse::<VaporId>().map_err(|error| error.to_string())
}

fn required_role(
    arguments: &mut impl Iterator<Item = OsString>,
    name: &str,
    usage: &dyn Fn() -> String,
) -> Result<VaporRole, String> {
    let value = required_argument(arguments, name, usage)?;

    let value = value
        .to_str()
        .ok_or_else(|| format!("{name} is not valid UTF-8"))?;

    value
        .parse::<VaporRole>()
        .map_err(|error| error.to_string())
}

fn reject_extra_arguments(
    arguments: &mut impl Iterator<Item = OsString>,
    usage: &dyn Fn() -> String,
) -> Result<(), String> {
    if arguments.next().is_some() {
        return Err("too many arguments\n\n".to_owned() + &usage());
    }

    Ok(())
}

fn vapor_usage() -> String {
    r#"usage:
    vapor installer role status
    vapor installer role promote <role>
    vapor installer role demote <role>

    vapor toolchain status
    vapor toolchain install

    vapor workspace status
    vapor workspace build
    vapor workspace test

    vapor discover <source-root>
    vapor resolve <source-root> <packagepack-id>
    vapor build <source-root> <packagepack-id>
    vapor run <source-root> <packagepack-id>"#
        .to_owned()
}

fn installer_usage() -> String {
    r#"usage:
    vapor-installer role status
    vapor-installer role promote <role>
    vapor-installer role demote <role>

roles:
    player
    composer
    content-developer
    ecosystem-developer
    root-authority

The same Installer operations are available through:
    vapor installer ..."#
        .to_owned()
}
