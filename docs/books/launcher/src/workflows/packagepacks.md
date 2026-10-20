# Packagepacks

A Packagepack is the authored root for a playable setup. It selects one engine
choice and one game choice. Either choice can be direct content or a reusable
Enginepack or Gamepack. The Packagepack can also declare compatible mods and
other dependencies.

The Packagepack is not itself the whole installed game. A Composition selects
a Packagepack release, stores the user's launch settings, and tracks the
resolved graph and installed state for that selection.

## Adding a Packagepack

When a Packagepack is available from Workshop or the local Library, choose
whether to create a new Composition from it or use it as the root of an
existing eligible Composition. Review the game and engine it selects, the
required versions, and its platform compatibility. Installing one root does
not change other Compositions.

## Choosing mods

Mods must target compatible content and satisfy their declared dependencies.
A Modpack groups mod content but does not replace the required engine and game
choices. The Launcher should show why a content item is eligible for a
Composition and explain a failed compatibility check in plain language.

## Source Packagepacks

A creator can edit a Packagepack in its registered Workspace. Saving a graph
checks required slots, dependency version requirements, cycles, and local
authoring closure. A locally authored Packagepack must have its constituent
and descendant content locally authored as well. This allows a creator to
modify a complete source composition without accidentally depending on hidden
Workshop payloads.

