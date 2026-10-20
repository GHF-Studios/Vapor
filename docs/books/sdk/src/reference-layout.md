# File Layout

Vapor keeps authored source, installed content, and generated state separate.

## Source workspace

Workspace.vapor.toml records source identity, registered content projects,
runtime targets, and workspace-level settings. Cargo.toml records Rust
workspace members. Child directories contain role manifests and source:

```text
my-content/
├── Workspace.vapor.toml
├── Cargo.toml
├── my-content-engine/Engine.vapor.toml
├── my-content-engine/Cargo.toml
├── my-content-game/Game.vapor.toml
├── my-content-game/Cargo.toml
└── my-content-packagepack/Packagepack.vapor.toml
```

The Packagepack is metadata-only. Actual folder names are generated from the
workspace slugs and may differ from this example.

## Managed app root

The installation contains app binaries, resources, installed docs, and managed
providers. Generated development build outputs are separate from source and
live in app-owned development roots. Installed content and Library cache are
managed by the shared service.

Treat state layout as diagnostic information, not an invitation to edit files
by hand. Use Vapor's source, content, root, and repair operations so indexes
and receipts remain consistent. The Internals book describes current paths for
maintainers.
