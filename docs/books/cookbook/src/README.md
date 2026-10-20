# The Vapor Cookbook

The Cookbook gives task-shaped paths through Vapor's current tools. Each recipe
states what it changes, the safe preview or validation step, and what success
looks like. It complements the concept books; it does not replace the full
command reference.

The shortest end-to-end authoring proof is
[create and locally deploy content](recipes/create-basic-packagepack.md).
For player operations, use
[install a Packagepack](recipes/install-packagepack.md) and
[lock and launch it](recipes/lock-and-launch.md). For distribution, start with
[the Workshop publication workflow](recipes/publish-to-workshop.md) or
[previewing the root app](recipes/package-root-app.md).

Recipes that depend on unavailable commands or providers say so explicitly.
In particular, source synchronization is not implemented by the current shell,
and binary-only Workshop artifacts cannot be turned into editable source by
forking.
