use vapor_core::{CliSurface, run_cli};

fn main() -> std::process::ExitCode {
    run_cli(CliSurface::Installer)
}
