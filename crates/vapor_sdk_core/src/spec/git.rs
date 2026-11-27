use crate::{GitCommand, StateSurface};

use super::{CommandSpec, spec};

pub(super) fn describe(command: &GitCommand) -> CommandSpec {
    match command {
        GitCommand::Build => spec(
            "sdk git build",
            "Build pinned upstream Git in Vapor's disposable Linux native-build cache.",
            StateSurface::BuildArtifact,
            &["x86-64 Linux host", "Vapor-managed Zig installed", "host libcurl and zlib development inputs available"],
            &["download and verify pinned Git and GNU Make sources", "compile with Vapor-managed Zig", "write a cache marker after Git version and inventory checks"],
        ),
    }
}
