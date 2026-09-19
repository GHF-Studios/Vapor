mod app;
mod connection;
mod model;

use clap::Parser;
use eframe::egui;
use std::process::ExitCode;
use std::sync::mpsc;

#[derive(Debug, Parser)]
#[command(
    name = "vapor-monitor",
    about = "Live Vapor Development Session observability dashboard"
)]
struct Cli {
    /// Connect directly to a telemetry broker instead of following the active Vapor session.
    #[arg(long)]
    address: Option<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let (sender, receiver) = mpsc::channel();
    connection::spawn_connection_worker(cli.address, sender);

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Vapor Monitor")
            .with_inner_size([1240.0, 820.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };

    match eframe::run_native(
        "vapor-monitor",
        options,
        Box::new(move |_creation| Ok(Box::new(app::MonitorApp::new(receiver)))),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("vapor-monitor: {error}");
            ExitCode::FAILURE
        }
    }
}
