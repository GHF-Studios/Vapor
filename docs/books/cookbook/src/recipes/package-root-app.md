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
Prepare the default Loo-Cast Engine, Game, and Packagepack content packages for
the same release targets first. Root staging selects only their runtime
manifests, Engine assets, and target executable under `content/default/`.
Missing inputs fail packaging. Additional content uses the Workshop lifecycle.

For an upload preview, use `root publish --dry-run`. Previewing is not an
upload; see [Publish The Root App](publish-root-app.md) for the explicit
confirmation boundary.
