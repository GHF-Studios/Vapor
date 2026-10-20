# The Vapor SDK Book

This book explains how to create and maintain Vapor source projects with the
Vapor app's managed developer tools. It is for people authoring engine, game,
mod, and Packagepack content, plus developers working on Vapor itself.

A **source workspace** is the authored project and its manifests. The
**installation root** is Vapor's managed runtime and toolchain. They are
separate on purpose: updating or repairing the installed app should not rewrite
your Git checkout, and a source project should not be mistaken for an installed
playable composition.

Start with [Quick Start](quick-start.md). Then read
[Workspaces](workflows/workspaces.md) and
[`Vapor.toml`](workflows/vapor-toml.md) to understand where projects are
registered. For code work, use the
[managed toolchain](workflows/toolchain.md) and
[managed Cargo](workflows/managed-cargo.md) commands. For content, follow
[Content Authoring](workflows/content-authoring.md) and
[Packs And Packagepacks](workflows/packs-and-packagepacks.md).

The CLI reference and [limits](limits.md) describe current behavior. Examples
here use Vapor commands and make no assumption that a system Rust installation
is available.
