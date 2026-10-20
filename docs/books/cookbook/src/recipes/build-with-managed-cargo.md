# Build With Managed Cargo

Build through Vapor so the pinned compiler, targets, environment, and app-owned
output location stay aligned.

1. Open the source root and inspect discovered Cargo workspaces:

   ```text
   source open /path/to/my-content
   source status
   ```

2. Build one workspace for iteration:

   ```text
   build --project my-content
   ```

3. Run the relevant managed checks:

   ```text
   check --project my-content
   test --project my-content
   validate --project my-content
   ```

The output is written under Vapor's development-output root rather than
scattered through source projects. Use `build --project all` only when you
intend to build every discovered workspace. Do not run system Cargo in
parallel for the same project: it may select a different compiler and cache.
See [Managed Cargo](../../sdk/workflows/managed-cargo.html) for the
ownership model.
