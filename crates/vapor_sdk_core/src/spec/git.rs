use crate::{GitCommand, StateSurface};

use super::{CommandSpec, spec};

pub(super) fn describe(command: &GitCommand) -> CommandSpec {
    match command {
        GitCommand::Build => spec(
            "sdk git build",
            "Build pinned upstream Git and static runtime dependencies in Vapor's disposable Linux build cache.",
            StateSurface::BuildArtifact,
            &["x86-64 Linux host", "Vapor-managed Zig installed", "host shell, Perl, tar, and GNU Make build prerequisites available"],
            &["download and verify pinned Git, curl, OpenSSL, zlib, and GNU Make sources", "compile a static x86-64 musl Git candidate with Vapor-managed Zig", "write a cache marker after Git version and full image inventory checks"],
        ),
    }
}
