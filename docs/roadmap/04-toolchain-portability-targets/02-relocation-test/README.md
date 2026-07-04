# Relocation Test

Status: needed next.

## Exists Now

- Toolchain state is intended to live under the app root.
- Rustup wrapping is provisional.
- No recorded relocation proof exists in the roadmap yet.

## Needed Next

- Copy or move the whole app root.
- Run SDK env/status/check/deploy from the moved root.
- Confirm `RUSTUP_HOME`, `CARGO_HOME`, `CARGO_TARGET_DIR`, and app-root paths point inside the moved root.
- Confirm unwanted system Cargo/Rustc/SteamCMD leakage is either absent or explicitly reported.

## Total Target

Relocation gives the evidence for keeping Rustup, repairing Rustup state, or returning to standalone Rust archives.
