# Current Rustup Backend

Status: partially implemented.

## Exists Now

- Toolchain pin comes from `Vapor/Vapor.toml`: nightly `2026-01-30`.
- SDK status reads Rustup from `VAPOR_HOME/tools/rustup/bin`, Rustup state from `VAPOR_HOME/tools/rustup/home`, and Cargo state from `VAPOR_HOME/tools/cargo/home`.
- SDK output and bootstrap state are owned under `VAPOR_HOME/data`.
- SDK prefers `$VAPOR_HOME/tools/rustup/bin/rustup`.
- If app-local Rustup is missing, SDK may use Rustup from PATH.
- `toolchain install` invokes Rustup with Vapor-owned `RUSTUP_HOME` and `CARGO_HOME`.
- Install verification checks the expected toolchain root plus `cargo` and `rustc` files.

## Needed Next

- Make already-installed toolchains an idempotent success or explicit repair path.
- Acquire/package app-local Rustup if Rustup remains the backend.
- Replace `present_unverified` with stronger verification.

## Total Target

The SDK owns the state roots, verifies what it installed, and never mistakes a random system toolchain for the Vapor toolchain.
