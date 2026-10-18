use std::{env, path::PathBuf, process};
use vapor_client_service::{
    ClientHostArguments, ClientHostConfig, ClientHostContext, HostPlatform,
    run_local_service, run_simulator, run_stdio_service,
};
use vapor_client::Client;
use vapor_client_protocol::LegacyStorageSeed;

fn main() {
    let mut args = env::args_os().skip(1);
    let first = args.next();
    if first.as_deref().is_some_and(|arg| arg == "simulate") {
        let mut scenario = None;
        while let Some(argument) = args.next() {
            if argument == "--scenario" {
                scenario = args.next().map(PathBuf::from);
            } else if argument == "--help" || argument == "-h" {
                println!("vapor-client-service simulate [--scenario FILE]");
                return;
            } else {
                eprintln!("unknown simulator argument: {}", argument.to_string_lossy());
                process::exit(2);
            }
        }
        match env::current_exe() {
            Ok(executable) => if let Err(error) = run_simulator(&executable, scenario.as_deref()) {
                eprintln!("Vapor Client simulator failed: {error}");
                process::exit(1);
            },
            Err(error) => {
                eprintln!("could not resolve simulator executable: {error}");
                process::exit(1);
            }
        }
        return;
    }

    let mode = first.as_deref().map(|arg| arg.to_string_lossy().into_owned());
    if mode.as_deref().is_some_and(|arg| arg == "--help" || arg == "-h") {
        println!("vapor-client-service (--ensure | --serve | --stdio) --application-executable PATH [--workshop-app-id ID] [--workspace PATH] [--state-dir PATH] [--active-composition ID] [--fixture]");
        return;
    }
    let mut application_executable = None;
    let mut workspace_root = None;
    let mut state_directory = None;
    let mut active_composition_id = None;
    let mut workshop_app_id = None;
    let mut fixture_mode = false;
    let mut ensure_mode = mode.as_deref() == Some("--ensure");
    let mut serve_mode = mode.as_deref() == Some("--serve");
    let mut stdio_mode = mode.as_deref() == Some("--stdio");
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--ensure" => ensure_mode = true,
            "--serve" => serve_mode = true,
            "--stdio" => stdio_mode = true,
            "--fixture" => fixture_mode = true,
            "--application-executable" => application_executable = args.next().map(PathBuf::from),
            "--workshop-app-id" => workshop_app_id = args.next().and_then(|id| id.to_string_lossy().parse().ok()),
            "--workspace" => workspace_root = args.next().map(PathBuf::from),
            "--state-dir" => state_directory = args.next().map(PathBuf::from),
            "--active-composition" | "--active-profile" => active_composition_id = args.next().map(|id| id.to_string_lossy().into_owned()),
            "--help" | "-h" => {
                println!("vapor-client-service (--ensure | --serve | --stdio) --application-executable PATH [--workshop-app-id ID] [--workspace PATH] [--state-dir PATH] [--active-composition ID] [--fixture]");
                return;
            }
            unknown => {
                eprintln!("unknown argument: {unknown}");
                process::exit(2);
            }
        }
    }
    if [ensure_mode, serve_mode, stdio_mode].into_iter().filter(|mode| *mode).count() != 1 {
        eprintln!("select exactly one of --ensure, --serve, or --stdio");
        process::exit(2);
    }
    let Some(application_executable) = application_executable else {
        eprintln!("--application-executable is required");
        process::exit(2);
    };
    let environment = env::vars_os().collect();
    let host_context = match ClientHostContext::resolve(
        ClientHostArguments { application_executable: application_executable.clone(), workspace_root: workspace_root.clone(), state_directory: state_directory.clone() },
        &environment,
        HostPlatform::current(),
    ) {
        Ok(context) => context,
        Err(error) => { eprintln!("Vapor Client service could not resolve its host context: {error}"); process::exit(2); }
    };
    let bootstrap = ClientHostConfig {
        application_executable,
        workshop_app_id,
        workspace_root,
        active_composition_id,
        fixture_mode,
        legacy_storage: LegacyStorageSeed::default(),
        state_directory,
    };
    let result = if ensure_mode {
        let executable = match env::current_exe() {
            Ok(executable) => executable,
            Err(error) => { eprintln!("could not resolve service executable: {error}"); process::exit(2); }
        };
        Client::ensure_service(&executable, bootstrap)
    } else if serve_mode {
        run_local_service(bootstrap, host_context).map_err(vapor_client::ClientError::Io)
    } else {
        run_stdio_service(bootstrap, host_context).map_err(vapor_client::ClientError::Io)
    };
    if let Err(error) = result {
        eprintln!("Vapor Client service failed: {error}");
        process::exit(1);
    }
}
