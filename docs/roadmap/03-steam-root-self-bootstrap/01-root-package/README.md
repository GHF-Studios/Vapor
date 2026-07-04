# Root Package

Status: partially implemented in SDK.

## Exists Now

- SDK constants define AppID `2122620` and DepotID `2122621`.
- `sdk root package` assembles `output/root/content`.
- It includes `sdk_cli`, `launcher_cli`, their binaries under `bin/`, and the activation script.
- It writes a SteamPipe app build VDF.
- It rejects AppID/DepotID zero.
- It does not include `cargo-home`, `rustup-home`, or `output`.

## Needed Next

- Decide whether docs/glossary files ship in the root package.
- Decide whether a toolchain ships in the package or is acquired after install.
- Decide whether missing Launcher output should block user-facing builds while Launcher is placeholder-heavy.

## Total Target

The root package is the Steam redistributable app root: enough CLI/app files to start Vapor workflows, without bundling local build outputs or caches accidentally.
