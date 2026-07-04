# Managed Cargo

Status: partially implemented.

## Exists Now

- SDK has `check`, `fmt`, and `build` commands.
- They run the Cargo binary from the Vapor toolchain root.
- They set `CARGO_HOME`, `RUSTUP_HOME`, `RUSTUP_TOOLCHAIN`, and `CARGO_TARGET_DIR`.
- They remove `RUSTC_WRAPPER`.
- They prefix PATH with Vapor toolchain/Rustup paths, then preserve existing PATH.

## Needed Next

- Decide which environment variables must be scrubbed.
- Decide whether preserving PATH is acceptable for now.
- Add status output that distinguishes app-local tools from PATH tools.

## Total Target

Managed Cargo should be honest: either hermetic by defined rules, or clearly labeled as managed-but-not-hermetic.
