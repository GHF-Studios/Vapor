# Lock And Launch

A Composition resolves to a Packagepack and a set of exact installed artifacts.
Inspect selection before launching:

```text
content selected
content verify
```

To set a Packagepack as the active composition:

```text
content select publisher/project/my-packagepack
```

The player's Launch action uses this same saved selection. The CLI can start
the selected composition with `launch selected`; `launch --current-workspace`
is a developer path for launching the active source workspace directly.

Treat an update as a deliberate change to the resolved graph. Verify the
installed content after updates and use the Composition's repair/update action
when dependency records need to be rebuilt. Do not manually edit generated
lock or receipt files under Vapor's app root; they are service-owned state.
