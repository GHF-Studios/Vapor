# Vapor.toml

Status: partially implemented.

## Exists Now

- `Vapor/Vapor.toml` identifies the core workspace and pins nightly `2026-01-30`.
- SDK, Launcher, Examples, and Registry each have workspace identity manifests.
- Examples use a root `[[content]]` graph.
- Generated example crates each have managed per-content `Vapor.toml`.
- `vapor_core` parses a strict root manifest shape for workspace identity and toolchain intent.

## Needed Next

- Decide the full active schema by workspace kind.
- Decide whether toolchain pins belong only to root/core or also other workspaces.
- Decide how registry metadata becomes validated.

## Total Target

`Vapor.toml` is human-authored intent, not generated state.
