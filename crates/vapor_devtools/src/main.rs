mod app;
mod connection;
mod model;

use clap::Parser;
use eframe::egui;
use std::process::ExitCode;
use std::sync::mpsc;

#[derive(Debug, Parser)]
#[command(
    name = "vapor-devtools",
    about = "Vapor runtime debugging, observability and performance-analysis workspace"
)]
struct Cli {
    #[arg(long)]
    address: Option<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let (sender, receiver) = mpsc::channel();
    connection::spawn_connection_worker(cli.address, sender);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Vapor Devtools")
            .with_inner_size([1480.0, 900.0])
            .with_min_inner_size([980.0, 620.0]),
        ..Default::default()
    };

    match eframe::run_native(
        "Vapor Devtools",
        options,
        Box::new(move |_creation| Ok(Box::new(app::DevtoolsApp::new(receiver)))),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Vapor Devtools: {error}");
            ExitCode::FAILURE
        }
    }
}
