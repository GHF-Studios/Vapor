# Toolchain Backend

Vapor pins the Rust toolchain used by its managed workflows. The toolchain
backend owns version selection, installation location, executable paths,
environment variables, target setup, and build output roots.

All Cargo execution paths should request that same policy. Workspace check,
format, tests, builds, Rustdoc, content realization, root release, and
developer scripts must not each construct their own toolchain environment.
This matters even when they invoke the same rustc: differing CARGO_HOME,
RUSTUP_HOME, target root, or linker policy creates separate caches and
inconsistent outputs.

The app root owns replaceable toolchain files; user source remains outside it.
Repair should restore the pinned version and its configured target policy
together. Cleanup should remove only Vapor-owned caches that it can identify.
Release profiles should omit full debug symbols unless a deliberate profiling
or debugging workflow requests them.
