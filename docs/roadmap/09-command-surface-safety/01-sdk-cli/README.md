# SDK CLI

Status: mixed real/stub.

## Exists Now

Real SDK command paths:

- `env`
- `workspace status`
- `workspace sync`
- `check`
- `fmt`
- `build`
- `deploy`
- `toolchain status`
- `toolchain install`
- `steam status`
- `steam login`
- `root package`
- `root publish`

Stub/spec surfaces:

- templates
- repairs
- packagepack/content authoring
- pack/leaf content workflows
- toolchain repair

## Needed Next

- Make placeholder output factual.
- Decide which command families become real next.
- Keep CLI help and `CommandSpec` text approval-gated when semantics change.

## Total Target

SDK CLI authors content, manages workspaces/toolchains, validates, packages, and publishes without hiding which flows are still unfinished.
