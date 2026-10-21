# Vapor Developer Environment

Vapor developer roles own a declared local tool capability set.

The rule is: feature commands consume developer capabilities; they do not invent
their own installation flows.

## Role ownership

`Composer` and higher currently require the pinned Vapor-managed
Rust/Cargo toolchain, Rust Analyzer/components, the pinned managed Tracy
profiler, and Tracy's managed saved-trace analysis utility.

Composer also receives the pinned Windows `gnullvm` Rust target and matching
UCRT-based LLVM-MinGW linker/runtime bundle, so Windows package builds are
available from supported developer installations without a second ad-hoc
setup path. Player installations remain free of these authoring tools.

Promotion reconciles this complete set before persisting the higher role.
Demotion is non-destructive and preserves installed tooling.

## Canonical reconciliation

All ordinary developer-tool provisioning converges on
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
