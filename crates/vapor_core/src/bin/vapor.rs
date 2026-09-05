use std::env;
use std::path::Path;
use std::process::ExitCode;

use vapor_core::{
    ContentVersionId, LocalCatalog, ManagedToolchain, ResolvedComposition, VaporId,
    build_cargo_realization, discover_local_content, generate_local_cargo_realization,
    resolve_local_packagepack, run_cargo_realization,
};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,

        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args_os();

    let _executable = arguments.next();

    let Some(command) = arguments.next() else {
        return Err(usage());
    };

    match command.to_str() {
        Some("toolchain") => {
            let action = required_argument(&mut arguments, "toolchain action")?;

            reject_extra_arguments(&mut arguments)?;

            match action.to_str() {
                Some("status") => toolchain_status(),
                Some("install") => toolchain_install(),

                Some(action) => Err(format!(
                    "unknown toolchain action `{action}`\n\n{}",
                    usage()
                )),

                None => Err("toolchain action is not valid UTF-8".to_owned()),
            }
        }

        Some("discover") => {
            let root = required_argument(&mut arguments, "source root")?;

            reject_extra_arguments(&mut arguments)?;

            discover(Path::new(&root))
        }

        Some("resolve") => {
            let root = required_argument(&mut arguments, "source root")?;

            let packagepack_id = required_vapor_id(&mut arguments, "Packagepack Vapor ID")?;

            reject_extra_arguments(&mut arguments)?;

            resolve(Path::new(&root), &packagepack_id)
        }

        Some("build") => {
            let root = required_argument(&mut arguments, "source root")?;

            let packagepack_id = required_vapor_id(&mut arguments, "Packagepack Vapor ID")?;

            reject_extra_arguments(&mut arguments)?;

            build(Path::new(&root), &packagepack_id)
        }

        Some("run") => {
            let root = required_argument(&mut arguments, "source root")?;

            let packagepack_id = required_vapor_id(&mut arguments, "Packagepack Vapor ID")?;

            reject_extra_arguments(&mut arguments)?;

            run_packagepack(Path::new(&root), &packagepack_id)
        }

        Some(command) => Err(format!("unknown command `{command}`\n\n{}", usage())),

        None => Err("command is not valid UTF-8".to_owned()),
    }
}

fn toolchain_status() -> Result<(), String> {
    let toolchain = ManagedToolchain::discover().map_err(|error| error.to_string())?;

    println!("Vapor-managed Rust toolchain:");
    println!(
        "  pin: {} {} ({})",
        toolchain.pin.channel, toolchain.pin.version, toolchain.pin.date,
    );
    println!("  home: {}", toolchain.vapor_home.display());
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
    arguments: &mut impl Iterator<Item = std::ffi::OsString>,
    name: &str,
) -> Result<std::ffi::OsString, String> {
    arguments
        .next()
        .ok_or_else(|| format!("missing {name}\n\n{}", usage()))
}

fn required_vapor_id(
    arguments: &mut impl Iterator<Item = std::ffi::OsString>,
    name: &str,
) -> Result<VaporId, String> {
    let value = required_argument(arguments, name)?;

    let value = value
        .to_str()
        .ok_or_else(|| format!("{name} is not valid UTF-8"))?;

    value.parse::<VaporId>().map_err(|error| error.to_string())
}

fn reject_extra_arguments(
    arguments: &mut impl Iterator<Item = std::ffi::OsString>,
) -> Result<(), String> {
    if arguments.next().is_some() {
        return Err("too many arguments\n\n".to_owned() + &usage());
    }

    Ok(())
}

fn usage() -> String {
    r#"usage:
    vapor toolchain status
    vapor toolchain install
    vapor discover <source-root>
    vapor resolve <source-root> <packagepack-id>
    vapor build <source-root> <packagepack-id>
    vapor run <source-root> <packagepack-id>"#
        .to_owned()
}
