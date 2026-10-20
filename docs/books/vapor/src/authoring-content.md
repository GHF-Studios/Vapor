# Authoring Content

Vapor content starts as ordinary, editable source in a Workspace. The root
`Workspace.vapor.toml` registers project paths; each registered project has a
role-specific manifest such as `Engine.vapor.toml` or
`Packagepack.vapor.toml`. The workspace registry decides which projects Vapor
recognizes. A stray manifest inside an unregistered directory is not
automatically added to the content graph.

## Start from a supported kind

Use a template that matches the content you intend to make. The basic-content
template creates an Engine, Game, and Packagepack workspace:

```text
source init basic-content /path/to/my-content --organization my-studio --name my-content
```

It creates a normal Cargo workspace for code-bearing projects, Vapor manifests
for the content roles, and explicit project registration. The Packagepack is
metadata-only; it selects the Engine and Game rather than duplicating their
source. Templates establish valid structure but do not hide the vocabulary:
the content role and dependency relationships remain visible in the manifests.

## Edit and validate

Open the new Workspace and inspect the resolved source context:

```text
source open /path/to/my-content
source status
content list
content validate
```

Add source projects through Vapor's create/adopt workflow so the Cargo workspace
and Vapor project registry remain aligned. Saving a Packagepack dependency graph
checks its role slots, version requirements, and cycles before replacing the
manifest. If a manifest changes outside Studio, refresh source discovery before
using the result for a build or upload.

## Run locally before sharing

Use a managed build or local deployment to check the content in the installed
application. These operations use Vapor's pinned Rust toolchain and app-owned
build outputs. Local deployment does not publish or change Workshop. It is a
separate proving step so source edits can be exercised before making a public
release.

For content with runtime targets, declare the intended matrix in
`Workspace.vapor.toml`. Omitting target flags then builds the declared matrix.
Host-only staging is useful for a local smoke pass, but it is not a valid
multi-platform Workshop release.

## Keep source and release state separate

The source project stays in its Workspace and Git repository. Packaging builds
the workspace, copies declared runtime outputs to a staged artifact, writes the
resolved deployed manifest, and fingerprints the staged files. The artifact is
self-describing runtime content; it is not a Cargo checkout and does not carry
the local build cache, authoring state, or credentials.

That separation lets you keep editing source while a player installs a stable
published version. It also allows Vapor to detect when a staged payload no
longer matches a review and to stop publication instead of uploading changed
bytes under an old review.

