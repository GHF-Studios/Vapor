# Deploy The SDK CLI

The installed CLI is part of Vapor's root application. Rebuild and promote it
through the root workflow rather than copying a Cargo binary into the app root.

For a local developer installation, use the host-only path:

```text
root deploy --host-only
```

This builds and deploys the locally declared root runtime for the current
machine. If you also want fresh installed documentation, do not pass
`--skip-docs`; that option is a faster binary-only refresh.

Before preparing a distributable app package, use the release-target matrix:

```text
root package
```

Packaging stages and smoke-checks the clean allowlisted app payload but does
not upload it. A real Steam release is a separate operation described in
[Publish The Root App](publish-root-app.md). Root deployment updates the local
Vapor installation; it does not publish a Workshop content artifact.
