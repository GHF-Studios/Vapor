# Command Surface And Safety Roadmap

Status: working roadmap.

Goal:
Keep command surfaces useful without pretending stubs are finished workflows.

## Map

1. [SDK CLI](01-sdk-cli/README.md)
2. [Launcher CLI](02-launcher-cli/README.md)
3. [Safety flags](03-safety-flags/README.md)

## Exists Now

- The SDK exposes broad command surfaces plus real environment, toolchain, workspace, root, and Steam slices.
- The Launcher is much less implemented.
- CLI help text, command summaries, preconditions, and future-effects text are documentation/policy surfaces.

## Needed Next

- Whether broad command stubs stay visible.
- Which command families must become real before further expansion.
- Whether current command names are disposable pre-stabilization scaffolding.
- How root SteamPipe publish and future Workshop content publish stay visibly separate.
- Whether machine-readable output is needed before GUI work.

## Total Target

The CLI remains usable for root work now and can later become a clean backend for GUI workflows.
