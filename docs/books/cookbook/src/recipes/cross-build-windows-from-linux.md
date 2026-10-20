# Cross-Build Windows From Linux

A cross-build uses Vapor's pinned Rust toolchain plus a Windows target and
linker configured by Vapor's managed development setup. Do not install an
unrelated system target and assume Vapor will find it.

Declare the target in the workspace's runtime target matrix, then use the
managed build path:

```text
content build --target x86_64-pc-windows-gnullvm
```

For a content artifact, package the declared matrix rather than creating a
separate Windows Workshop item:

```text
content package my-studio/my-content/my-content-packagepack
```

The package should contain one artifact root with target-specific binaries
under `bin/x86_64-pc-windows-gnullvm/`. A local build proves compilation, not
that the game starts on Windows; test the staged artifact on a Windows machine
before release. If the linker or target is missing, use the managed toolchain
setup/repair path and inspect its diagnostic rather than modifying Vapor's
toolchain directory by hand.
