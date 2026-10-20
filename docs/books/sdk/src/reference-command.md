# Command Reference

The interactive Vapor Shell is the command-line facade over the same local
Vapor service used by graphical applications. Its complete command syntax and
per-command side effects are maintained in the
[Shell command guide](https://vapor.ghf-studios.site/docs/shell/commands.md).

For authoring, the common sequence is:

```text
source open /path/to/workspace
source status
content list
content validate
```

For managed development:

```text
check --project all
fmt --project all
test --project all
build --project all
validate --project all
```

For local content iteration, use `content deploy ARTIFACT --select --host-only`.
For release content, inspect `content package ARTIFACT --dry-run` and the
Workshop create/update workflow before any real upload.

Command references explain accepted arguments; the surrounding SDK pages cover
ownership and safety semantics. In particular, `source sync` is currently a
status hook and does not synchronize a Git checkout.
