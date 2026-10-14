//! SDK toolchain command specifications.

use super::{CommandSpec, StateSurface, spec};
use crate::toolchain::ToolchainCommand;

pub(super) fn describe(command: &ToolchainCommand) -> CommandSpec {
    match command {
        ToolchainCommand::Status => spec(
            "sdk toolchain status",
            "Inspect pinned Rust/Cargo/toolchain state.",
            StateSurface::ReadOnly,
            &[],
            &["display pinned toolchain state"],
        ),
        ToolchainCommand::Install => spec(
            "sdk toolchain install",
            "Install the pinned Rust/Cargo toolchain through Rustup.",
            StateSurface::Toolchain,
            &[
                "toolchain pin is known",
                "Rustup is available from VAPOR_HOME/rustup/bin or PATH",
            ],
            &[
                "invoke Rustup with Vapor-owned RUSTUP_HOME and CARGO_HOME",
                "install required components and target standard libraries",
                "leave Rustup responsible for Rust distribution details",
            ],
        ),
        ToolchainCommand::Repair => spec(
            "sdk toolchain repair",
            "Repair the pinned Rust/Cargo toolchain installation.",
            StateSurface::Toolchain,
            &["toolchain pin is known"],
            &["repair or reinstall damaged toolchain components"],
        ),
    }
}
