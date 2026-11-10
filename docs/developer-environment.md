# Vapor Developer Environment

Vapor developer roles own a declared local tool capability set.

The rule is: feature commands consume developer capabilities; they do not invent
their own installation flows.

## Role ownership

The `Developer` role requires the pinned Vapor-managed Rust/Cargo toolchain,
Rust Analyzer/components, both supported Rust targets, both cross-linker
toolsets, the pinned managed Tracy profiler, and Tracy's saved-trace analysis
utility. Developer setup is mandatory; the Installer does not offer a partial
or skipped toolset.

For Linux package builds, Vapor manages Zig and a target-normalizing compiler
wrapper. For Windows GNU builds, it manages the matching UCRT LLVM-MinGW
compiler, archiver, and runtime. These tools and Rustup state are owned by the
Vapor installation. Player installations remain free of these authoring tools.

Promotion reconciles this complete set before persisting the higher role.
Demotion is non-destructive and preserves installed tooling.

## Canonical reconciliation

All Developer-tool provisioning converges on
`DeveloperEnvironment::reconcile`.

It is used by role promotion, `vapor toolchain install`,
`vapor toolchain repair`, and broad `vapor installation repair`.

## Consumption

Consumers never install tools implicitly. `vapor toolchain cargo` executes the
managed Cargo toolchain, `vapor toolchain tracy` launches managed Tracy,
Devtools uses managed Tracy analysis for reports, and `vapor run --profiling`
only runs the profiling configuration.

If a required tool disappears, diagnosis reports the developer environment as
degraded and repair restores the declared state.
