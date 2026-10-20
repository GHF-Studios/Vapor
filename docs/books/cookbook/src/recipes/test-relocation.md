# Test Relocation

A relocation check asks whether an assembled Vapor app still launches after it
is moved away from the checkout and build environment. This is different from
testing source discovery or Steam integration.

1. Assemble a host-only local package:

   ```text
   root package --host-only
   ```

2. Use the package path reported by Vapor and copy the complete staged root to
   a fresh directory. Do not copy only the executable: runtime scripts,
   manifests, docs, resources, and platform binaries belong together.
3. Launch the staged entrypoint from that new location using the platform's
   stable entrypoint. Confirm that it discovers its app root and reaches the
   expected mode.
4. Repeat without the source checkout available if the test is intended to
   prove a player installation is self-contained.

Record missing files and attempted source-tree paths as defects. A successful
build alone does not prove relocation. The complete cross-platform package
must be tested on each target operating system.
