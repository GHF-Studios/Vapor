//! Command specifications for app-root shell environment discovery.

use crate::environment::EnvironmentCommand;

use super::{CommandSpec, StateSurface, spec};

pub(super) fn describe(command: &EnvironmentCommand) -> CommandSpec {
    match command {
        EnvironmentCommand::Status => spec(
            "sdk env",
            "Show the app-root shell environment Vapor expects humans to activate.",
            StateSurface::Environment,
            &[],
            &[
                "display the app-root activation script",
                "display process environment variables and PATH entries used by Vapor tools",
            ],
        ),
    }
}
