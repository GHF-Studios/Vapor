# Content Types

The content vocabulary is defined by the Vapor Rust crates. Authors select a
supported kind in a role-specific manifest; they do not define new kinds in
TOML or Rhai. Each kind has a stable spelling and manifest filename. A template
may create a project of one of these kinds, but a template does not change what
the kind means.

| Kind | Manifest | Purpose |
| --- | --- | --- |
| `engine` | `Engine.vapor.toml` | A runtime engine and its declared dependencies and runtime outputs. |
| `game` | `Game.vapor.toml` | Game content that selects an engine and may depend on crates. |
| `packagepack` | `Packagepack.vapor.toml` | A complete playable root that selects one engine choice and one game choice. |
| `enginepack` | `Enginepack.vapor.toml` | A reusable engine choice that resolves to an engine. |
| `gamepack` | `Gamepack.vapor.toml` | A reusable game choice that resolves to a game. |
| `modpack` | `Modpack.vapor.toml` | A reusable grouping of compatible mods. |
| `engine-mod` | `Engine-Mod.vapor.toml` | A modification that targets engine behavior. |
| `game-mod` | `Game-Mod.vapor.toml` | A modification that targets game behavior. |
| `extension-mod` | `Extension-Mod.vapor.toml` | A mod that extends supported content without replacing an engine or game. |
| `crate` | `Crate.vapor.toml` | Rust library source and dependency metadata, distinct from runtime content. |

The names describe roles in a dependency graph. An `engine` or `game` is
code-bearing content. An `enginepack`, `gamepack`, `modpack`, or
`packagepack` is a composition object that can select or group other content.
The exact legal parent/child relationships and required slots are checked by
Vapor's shared content model, not invented by each front end.

## Crate versus Library

`crate` is the content kind for Rust library source. It is intended for static
composition into the code-bearing application content and does not represent a
download cache. The product's **Library** is the user-facing collection of
content available locally: Workshop-acquired cache entries, installed content,
and other discovered local copies. One is an authored Rust artifact type; the
other is a local content lifecycle view.

## Identity and version

A content ID identifies the same authored item across versions. A content
release is one ID at one semantic version. Dependencies refer to an ID plus a
version requirement, with optionality recorded explicitly. Resolving a graph
selects concrete releases that satisfy those requirements. The composition lock
then records what was selected, so later discovery of a newer release does not
silently rewrite the installed composition.

For a new project, use a Vapor template rather than copying a manifest from an
unrelated kind. The template creates the correct Cargo project shape,
role-specific manifest, and explicit workspace registration for that kind.

