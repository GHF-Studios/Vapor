# Quick Start

This path creates a minimal authored content workspace, validates its content
graph, and locally deploys a playable Packagepack. It assumes Vapor is installed
and you have a usable local app root.

## Create a source workspace

Choose a path that does not contain files you need to preserve. The built-in
template creates an Engine, a Game, and a Packagepack, registers them in
`Workspace.vapor.toml`, adds code-bearing projects to the Cargo workspace,
and opens the new source root:

```text
source init basic-content ~/Vapor/Projects/my-content --organization my-studio --name my-content
```

The path may be elsewhere; authored projects do not need to live under the
installed app. The organization and name become part of the stable content IDs.
Use lowercase slugs with hyphens.

## Validate and deploy locally

```text
content validate
content deploy my-studio/my-content/my-content-packagepack --select --host-only
```

Validation checks the registered content and its relationships. Host-only
deploy is a local smoke path; it stages only the current machine's target and
does not prove that the package is ready for cross-platform publication. The
packagepack becomes the selected playable composition when deployment
succeeds.

Use `content status` or `content list` to inspect the result. Source files
remain in the workspace; deployed files are staged under the Vapor app root.
When the first local launch works, continue with
[Packagepacks](workflows/packs-and-packagepacks.md) and the
[Workshop publishing guide](workflows/workshop.md) before publishing anything.
