# Package The Root App

Root packaging assembles the application/depot payload and runs local
structural smoke checks without uploading it.

First ensure the declared runtime target matrix is configured and build the
release package:

```text
root package
```

For a local smoke package, restrict the operation to the host:

```text
root package --host-only
```

The default release path builds the root applications, stages the common and
platform depot contents, and checks required entrypoints and runtime files.
Inspect the resulting package report before using it as a release candidate.
This package contains Vapor's launcher/runtime tools and docs; game content
such as Loo-Cast is published through its own content/Workshop lifecycle.

For an upload preview, use `root publish --dry-run`. Previewing is not an
upload; see [Publish The Root App](publish-root-app.md) for the explicit
confirmation boundary.
