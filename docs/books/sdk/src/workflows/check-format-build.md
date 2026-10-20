# Check, Format, Build

Vapor routes Cargo work through its pinned toolchain and managed output policy.
Use these commands from the interactive Shell with the intended source root
open:

```text
source open /path/to/workspace
fmt --project all
check --project all
test --project all
build --project all
```

Use a named project for a smaller iteration. Available names come from Cargo
workspace discovery; all selects every discovered workspace under the active
source root.

Validate is the broader gate: formatting check, Cargo check, tests, strict
Clippy, and strict Rustdoc. Use it before handing off code changes. Compilation
output is written to Vapor-managed roots rather than into each source package.

Content has a separate validation layer. After Cargo checks, run content
validate to verify role manifests, dependencies, conflicts, and composition.
A successful code build does not prove a Packagepack is valid, and a valid
content graph does not prove the code compiles.
