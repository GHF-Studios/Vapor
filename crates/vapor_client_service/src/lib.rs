//! Process-owned application state and the shared local IPC service host.

mod catalog;
mod host;
mod legacy_import;
mod persistence;
mod scenario;
mod server;
mod session;
mod simulator;

pub use host::{HostPlatform, ClientHostArguments, ClientHostContext};
pub use server::{run_local_service, run_stdio_service};
pub use session::ClientSession;
pub use vapor_client::ClientHostConfig;
pub use simulator::run_simulator;
