//! Reusable diagnostics view and telemetry connection for Vapor Studio.

pub mod app;
mod connection;
mod model;

/// Create a diagnostics view connected to the active Vapor runtime session.
pub fn connect(address: Option<String>) -> app::DevtoolsApp {
    let (sender, receiver) = std::sync::mpsc::channel();
    connection::spawn_connection_worker(address, sender);
    app::DevtoolsApp::new(receiver)
}
