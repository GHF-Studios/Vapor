# Local Development

Local development uses the same Vapor content model as distribution, but keeps
editable source outside the installed app. This lets a developer build and run
source without replacing Steam-installed files or confusing a checkout with a
published release.

## Workspaces and the Superworkspace

A Vapor Workspace has its own `Workspace.vapor.toml`, Cargo workspace, and
registered projects. `source open PATH` selects it for a Shell session. A
Vapor Superworkspace is the developer's collection of repositories; it is a
place to discover, clone, and edit source, not a second semantic content
registry. The app root remains the owner of installed runtimes, provider tools,
downloads, and managed build artifacts.

Open a repository directly when you know its path:

```text
source open /path/to/Loo-Cast
source status
metadata
```

When the same source is registered in more than one workspace, use its
repository identity and selected source context rather than relying on the
current directory to guess which project you meant.

## Managed development operations

Use Vapor Shell operations to format, check, test, build, and validate declared
Cargo workspaces. Vapor selects the pinned toolchain and app-owned cache roots.
The generated artifacts are stored under the application output area, not
inside the source repository. `clean` clears a selected development cache while
preserving release outputs, documentation, installed content, and source.

An omitted target uses the workspace runtime matrix when it declares one.
Host-only output is an explicit fast path for local iteration. Do not use it as
proof that a release can run on every declared platform.

## Local content versus app-root publication

Use a local deploy or content roundtrip to exercise the source in an installed
app. This changes only the local test instance. A Steam app-root publication is
a separate release action with its own review, target validation, and explicit
confirmation. A Workshop content release is another separate operation tied to
the selected content artifact. Local success never implies that either remote
publication happened.

For scripts and automation, use the Vapor Shell grammar or a checked-in
`resources/vapor/vapor-scripts/NAME.vapor` script. That keeps automation on the
same typed operation path as an interactive session.

