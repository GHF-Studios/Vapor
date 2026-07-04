# Active Repos

Status: exists now.

## Exists Now

- `Vapor` owns root ecosystem concepts and the `vapor_core` / `vapor_macros` crates.
- `Vapor-SDK` owns `vapor_sdk_core` and `vapor_sdk_cli`.
- `Vapor-Launcher` owns `vapor_launcher_core` and `vapor_launcher_cli`.
- `Vapor-Examples` is a custom-content example workspace with engine, game, and packagepack entries.
- `Vapor-Registry` has a registry manifest and describes GitHub-hosted identity/release authority.

## Needed Next

- Decide exactly when `Vapor-Registry` becomes a hard release authority.
- Decide whether SDK/Launcher should use Git/branch dependencies, path dependencies, or published crates at each stage.
- Keep repo-local docs aligned with repo ownership.

## Total Target

- Vapor defines shared concepts and core types.
- SDK authors, validates, builds, packages, and publishes.
- Launcher installs, composes, locks, and launches.
- Examples prove external authoring flows.
- Registry gives public, reviewed identity and permission metadata.
