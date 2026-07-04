# Safety Flags

Status: shared surface exists.

## Exists Now

- SDK and Launcher expose `--yes`, `--force`, `--strict`, and `--keep-unused-versions`.
- SDK also exposes `--workspace` and `--verbose`.
- Safety guards exist before command handling.

## Needed Next

- Define `--force` only per real mutation.
- Decide prompt behavior for sync, fmt, deploy, install, uninstall, publish, and repair.
- Decide if/when machine-readable output is required for GUI use.

## Total Target

Mutation commands are clear about what they change, when they prompt, and what each safety flag permits.
