//! Shared frontend client and bootstrap manager for Vapor Client's local service.

mod host;
pub mod local_socket;
mod process;
mod transport;

pub use host::{ClientHostArguments, ClientHostContext, HostPlatform};
pub use local_socket::endpoint_name;
pub use process::{Client, ClientError, ClientHostConfig, service_sibling_path};
pub use vapor_client_domain::*;
