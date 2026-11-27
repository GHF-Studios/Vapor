# Vapor Developer Environment

Vapor developer roles own a declared local tool capability set.

The rule is: feature commands consume developer capabilities; they do not invent
their own installation flows.

## Role ownership

The `Developer` role requires Vapor-managed Git, the pinned Rust/Cargo toolchain,
Rust Analyzer/components, both supported Rust targets, both cross-linker
toolsets, the pinned managed Tracy profiler, and Tracy's saved-trace analysis
utility. Developer setup is mandatory; the Installer does not offer a partial
or skipped toolset.

For Linux package builds, Vapor manages Zig and a target-normalizing compiler
wrapper. For Windows GNU builds, it manages the matching UCRT LLVM-MinGW
compiler, archiver, and runtime. These tools and Rustup state are owned by the
Vapor installation. Player installations remain free of these authoring tools.

Git is resolved through the explicit Vapor provider boundary. When no working
provider is explicitly linked, Developer setup authenticates the stable release
manifest, downloads its signed Git payload, validates the archive and runtime,
then atomically activates it under `Vapor User Data/tools/git`. The archive
reader preserves contained hard links used by Git's command aliases; it rejects
symbolic links, traversal, and unsupported entry types. Git is not part of
Player setup or the replaceable Steam App Instance. The standalone Installer
checks this mandatory capability before downloading the larger Developer tool
bundle, so a missing signed Git asset fails early without leaving those costly
tools half-installed.

Promotion reconciles this complete set before persisting the higher role.
Demotion is non-destructive and preserves installed tooling.

## Reconciliation ownership

`DeveloperEnvironment::reconcile` owns Core's declared developer capability set.
Role promotion, `vapor toolchain install`, `vapor toolchain repair`, and broad
`vapor installation repair` use that operation.

The standalone Installer's `dev-env install` still uses its precompiled
maintenance setup sequence for SteamCMD, Rust, and cross-build tools. It calls
the same Core `managed_git::ensure_installed` operation for Git and writes its
ready receipt only after that succeeds. Git acquisition and trust are shared;
the remaining Developer provisioning sequence has not yet converged on one
reconciler.

## Consumption

Consumers never install tools implicitly. `vapor toolchain cargo` executes the
managed Cargo toolchain, `vapor toolchain tracy` launches managed Tracy,
Devtools uses managed Tracy analysis for reports, and `vapor run --profiling`
only runs the profiling configuration.

If a required tool disappears, diagnosis reports the developer environment as
degraded and repair restores the declared state.
