# Deploy

Deployment promotes built files into a Vapor-owned local app or content root.
It is different from building source and from publishing to a remote service.

For Vapor's own app-root projects:

```text
root deploy --host-only
```

For authored game content:

```text
content deploy ORG/NAME/NAME-packagepack --select --host-only
```

These host-only paths are for local iteration. They do not create a
cross-platform release or upload anything to Steam. Omit host-only only when
you intend to use the workspace's declared runtime target matrix.

Root deploy can rebuild installed documentation. Use skip-docs only for a
faster binary or script refresh when the docs tree does not need updating.
Inspect the operation result and run the deployed app or composition before
claiming a local deployment works.
