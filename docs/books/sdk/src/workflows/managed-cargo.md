# Managed Cargo

Vapor runs Rust and Cargo through its pinned, app-managed toolchain. This keeps
the compiler version, target setup, environment, and output policy consistent
across the SDK's build, test, documentation, and packaging operations.

The SDK command surface includes `sdk build-release` for an optimized
workspace build. For example, CI can build one declared workspace with:

```text
vapor sdk --workspace /path/to/workspace build-release
```

Its artifacts remain in Vapor User Data; it does not deploy them into an App
Instance or publish them. Check, build, release-build, and test operations use
Cargo's `--locked` option so they cannot rewrite an authored `Cargo.lock`.
Use the explicit lockfile-resolution operation when dependency versions should
change.

Use the Vapor command for the job:

```text
check --project my-workspace
fmt --project my-workspace
test --project my-workspace
build --project my-workspace
validate --project my-workspace
```

The project name is a Cargo workspace discovered from the active source. Use
`all` when you intentionally want every workspace selected by the active root.
For a source root, Cargo outputs go to Vapor-managed development output paths,
not the source tree. Root-app and content workflows may promote or package
those products through their dedicated commands.

This command boundary is more than selecting a compiler executable. Vapor
also owns the toolchain's installation and target-root policy. Avoid running a
second Cargo path for the same project: it can create a separate cache, use a
different toolchain, or produce files Vapor's deployment path does not know
about.

Use `clean` only when you want Vapor to remove managed development build
outputs for the active workspace. It preserves release outputs, docs,
installed content, and source files. For complete command behavior, see the
[command reference](../reference-command.md).
