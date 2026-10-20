# Repair Local State

Use the narrowest repair that matches the problem. Repair does not mean delete
all local data.

For source metadata, preview safe repairs first:

```text
source repair
```

If the proposed dependency Workshop IDs are correct, apply them explicitly:

```text
source repair --write
```

For installed artifacts, inspect and verify before repairing:

```text
content status
content verify
content repair
```

With no artifact selector, content repair checks the installed set and
reconstructs damaged items from the active source or available cache/provider
material. Review the reported actions and failure details. Source repair and
installed-content repair affect different data; neither is a substitute for a
backup of authored source.
