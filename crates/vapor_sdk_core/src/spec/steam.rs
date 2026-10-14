//! Command specifications for SteamCMD tooling.

use crate::steam::types::SteamCommand;

use super::{CommandSpec, StateSurface, spec};

pub(super) fn describe(command: &SteamCommand) -> CommandSpec {
    match command {
        SteamCommand::Status(_) => spec(
            "sdk steam status",
            "Inspect SteamCMD availability and Vapor Steam state paths.",
            StateSurface::Steam,
            &[],
            &[
                "display the SteamCMD executable Vapor would call",
                "display the app-root Steam state directory reserved for Vapor tooling",
            ],
        ),
        SteamCommand::Login(_) => spec(
            "sdk steam login",
            "Run SteamCMD login without Vapor seeing the password or Steam Guard code.",
            StateSurface::Steam,
            &["SteamCMD must be available through --steamcmd or PATH"],
            &[
                "invoke SteamCMD with +login and +quit",
                "let SteamCMD prompt for password and Steam Guard if needed",
                "let SteamCMD persist its own login token/config according to SteamCMD behavior",
            ],
        ),
    }
}
