# Toolchain Portability And Targets Roadmap

Status: working roadmap.

Goal:
Make the Rust/Cargo toolchain app-local enough for Steam-root development.

## Map

1. [Current Rustup backend](01-current-rustup-backend/README.md)
2. [Relocation test](02-relocation-test/README.md)
3. [Targets](03-targets/README.md)

## Exists Now

- The SDK wraps Rustup with Vapor-owned `RUSTUP_HOME` and `CARGO_HOME`.
- The SDK prefers app-local `rustup/bin/rustup` when present.
- The current backend may still use PATH Rustup during development bootstrap.
- Build output is kept under Vapor-managed output paths.

## Needed Next

- The exact relocation test.
- Whether PATH Rustup is acceptable only for development bootstrap.
- Whether `toolchain install` should become idempotent around an existing app-local toolchain.
- Whether Windows GNU is the first Windows cross-build proof target.
- How to separate host triples, installed stdlib targets, linkable build targets, and release-supported targets.

## Total Target

The SDK can install, verify, repair, relocate, and use the expected Rust/Cargo toolchain without silently depending on the developer's normal machine setup.
