# Source Tree

The Vapor shared repository is a Cargo workspace whose packages live under
crates. Packages are grouped by ownership: semantic content and manifests,
client domain/protocol/application/service/provider, toolchain and diagnostics,
Steam integration, and SDK command specifications.

Vapor-Client assembles application roots and pins the managed toolchain.
Vapor-CLI contains Clap command grammar and shell presentation. Vapor-GUI
provides GUI bootstrapping and host applications. Vapor-Studio contains the
current creator/player GUI workflows. The entrypoint repository selects a
shipped mode and starts the local service/frontend path.

Before changing a boundary, trace the full request path:

1. Which frontend accepts the user's intent?
2. Which typed protocol action represents that intent?
3. Which service module owns validation and state mutation?
4. Which provider performs external I/O?
5. Which snapshot/event reports progress and result to the frontend?

The repository layout changes over time. Use the current Cargo workspaces and
Rust module declarations as authority; do not rely on an old diagram or
directory name.
