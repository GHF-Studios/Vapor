//! Command specifications for root package and SteamPipe publishing.

use crate::root::types::RootCommand;

use super::{CommandSpec, StateSurface, spec};

pub(super) fn describe(command: &RootCommand) -> CommandSpec {
    match command {
        RootCommand::Package(request) => spec(
            if request.plan {
                "sdk root package --plan"
            } else {
                "sdk root package"
            },
            "Assemble the Steam redistributable root package and SteamPipe app build script.",
            StateSurface::RootRelease,
            &[
                "the SDK executable root must contain bin/ and Vapor activation script(s)",
                "the package output is generated under the SDK executable root output/ directory",
                "cargo-home/ and output/ are never included in the Steam redistributable content",
            ],
            &[
                "refresh the managed root package content directory",
                "write a SteamPipe app build VDF for the configured AppID and DepotID",
            ],
        ),
        RootCommand::Publish(request) => spec(
            if request.plan {
                "sdk root publish --plan"
            } else {
                "sdk root publish"
            },
            "Assemble the root package and upload it through SteamCMD.",
            StateSurface::RootRelease,
            &[
                "the Steam account must have permission to upload the configured AppID and DepotID",
                "SteamCMD must be available through --steamcmd or PATH",
                "SteamCMD may prompt for password and Steam Guard when no login token exists",
            ],
            &[
                "run SteamCMD with +login and +run_app_build",
                "upload the generated root package through SteamPipe",
                "leave Steamworks branch activation manual unless --set-live names a beta branch",
            ],
        ),
    }
}
