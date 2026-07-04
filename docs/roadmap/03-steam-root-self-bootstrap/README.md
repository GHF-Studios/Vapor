# Steam Root And Self-Bootstrap Roadmap

Status: working roadmap.

Goal:
Make the Steam-distributed app root able to maintain and publish itself.

## Map

1. [Root package](01-root-package/README.md)
2. [Root publish](02-root-publish/README.md)
3. [Self-bootstrap](03-self-bootstrap/README.md)

## Exists Now

- Steam AppID: `2122620`.
- Steam DepotID: `2122621`.
- The SDK has root package and root publish flows through SteamCMD.
- The current root package includes app-root CLI surfaces and activation scripts.
- The current root package excludes `rustup-home`, `cargo-home`, and `output`.

## Needed Next

- Whether canonical docs and glossary material ship in the Steam root package.
- Whether the root package ships a full app-local Rust toolchain or acquisition/reconciliation logic.
- What proof completes the self-bootstrap loop.
- Whether `sdk_cli` and `launcher_cli` are long-term command names or current aliases.

## Total Target

Steam downloads the app root, the downloaded SDK can build/check/package/publish the next root, and normal player installs do not rely on developer hot-deploy behavior.
