# Engines, Games, And Mods

An Engine supplies a runtime. A Game supplies the game-specific content that
runs on a compatible Engine. A mod changes or extends one of those authored
systems. A pack groups or selects content so that users can reuse a known-good
combination.

| Role | What it represents | Common parent or selector |
| --- | --- | --- |
| Engine | A runtime and declared runtime files | Packagepack or Enginepack |
| Game | A game implementation and its engine dependency | Packagepack or Gamepack |
| Engine Mod | A change that targets an engine | Modpack or Packagepack |
| Game Mod | A change that targets a game | Modpack, Gamepack, or Packagepack |
| Extension Mod | A supported extension that is neither an engine nor game replacement | Modpack or Packagepack |

Engine and Game are not synonyms for an installation directory or source
repository. They are typed content roles in the resolved graph. The Engine
declares runtime binaries and libraries so that packaging can stage those files
into platform-specific locations. The Game declares which engine relationship
it requires. Vapor validates that relationships before installing or launching
the resulting composition.

Mods use dependency requirements to state what they target and need. A Modpack
can group compatible modifications for reuse, but it does not replace the
required Engine or Game choice in a Packagepack. The Packagepack remains the
playable root that ties an engine choice and a game choice together.

The role names and direct dependency rules are defined in Rust. If a proposed
relationship is not supported by the content model, authors cannot make it
valid merely by adding a new manifest field. That is an engine/API decision,
not a content-local convention.

