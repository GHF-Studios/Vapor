# The Vapor Book

Vapor is the shared system for describing, assembling, distributing, and
running versioned game content. Its player Launcher, creator Studio,
code-authoring IDE, and CLI/Shell use one vocabulary and one set of lifecycle
rules. A content item
has an identity and kind; a workspace is its editable source; a release is a
packaged version; and a composition describes what the player intends to run.
Those objects connect, but they are not interchangeable.

This book explains the current Vapor model and points out where a workflow is
still a preview or requires developer setup. The [small tour](small-tour.md)
starts with real commands. The [ecosystem map](concepts/ecosystem-map.md) shows
how the shared Client service, Studio, Steam, and server services fit together.
Use [Content Types](concepts/content-types.md) and
[Packs And Composition](concepts/packs-and-composition.md) before authoring
content; they explain why a playable setup is a graph rather than a folder of
untyped downloads.

## The shortest useful model

- **Workspace:** editable source, registered explicitly in a
  `Workspace.vapor.toml` manifest.
- **Content:** an engine-defined role such as `game`, `engine-mod`, or
  `packagepack`, with identity, version, and declared dependencies.
- **Release:** the built, self-describing runtime payload for one version. It
  is staged and fingerprinted before distribution.
- **Library:** content available on this device. It can include cached releases
  that are not part of the selected composition.
- **Composition:** the player's launchable setup: a root Packagepack and its
  resolved content graph, plus launch settings.
- **Installation:** the verified local files for a composition's resolved
  graph. It is derived from the selected versions and can be repaired.

The distinction matters in ordinary use. Editing a workspace does not silently
change the installed game. Downloading an item makes it available locally; it
does not automatically change every composition. Updating a composition is a
reviewable resolution/install operation, not a blind replacement of arbitrary
files.

## Current scope

Vapor Client already owns local discovery, Workshop search and subscriptions,
download and cache acquisition, composition resolution, install/update/repair,
managed builds, package staging, and reviewed Workshop publication. The GUI and
Shell send typed actions to the shared Client service; they do not implement
their own package resolver or Steam file transfer. The service and platform
providers make the result available to both surfaces.

The product books and in-game authoring experience are still being filled in.
Do not mistake a listed goal for a finished feature: the current state of each
workflow is described in its relevant chapter, and executable Shell behavior is
documented in the deployed [Shell command guide](https://vapor.ghf-studios.site/docs/shell/commands.md).
