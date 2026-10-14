//! SteamCMD tooling and authentication workflows.
//!
//! Vapor never accepts or stores Steam passwords. Login delegates to SteamCMD so
//! SteamCMD owns password prompts, Steam Guard, and its own persisted login
//! token/config behavior.

pub mod error;
pub mod process;
pub mod status;
pub mod types;
