# Workspaces

Vapor uses a source root to define a set of related authored projects. The root
contains `Workspace.vapor.toml` and usually a Cargo workspace manifest. Vapor
does not treat every nested folder as content: a project becomes part of the
Vapor content graph when it is explicitly registered in the workspace
manifest.

## Open and inspect a workspace

Use an absolute path the first time:

```text
source open /path/to/my-content
source status
content list
```

Opening validates the workspace, indexes it for this installation, and makes
it the active source for source-backed commands. Later you can select it by its
registered name. `source list` shows indexed roots; `source close` clears the
active workspace without removing it from the index.

## Registration has two roles

`Workspace.vapor.toml` records Vapor projects and runtime targets.
`Cargo.toml` records Rust packages and workspace members. Engine and Game
projects usually have both a Vapor content registration and a Rust package.
Packagepack is metadata-driven and does not need to be a Cargo crate. Keeping
these registries aligned is why content creation and adoption go through
Vapor rather than ad-hoc folder copying.

Use `source repair` to inspect safe repairs. It previews by default;
`source repair --write` applies supported source metadata repairs. Review
the proposed actions before writing them.

## Scope and safety

A source root must be outside the installed app root. Once opened, source
operations are scoped to that root. Unregistered project folders may be
adopted; unregistering a project removes its workspace registration but leaves
its files and Cargo membership in place. This makes discovery reversible
without treating deletion as cleanup.
