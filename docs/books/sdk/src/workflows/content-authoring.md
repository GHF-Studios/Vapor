# Content Authoring

A Vapor source workspace may contain multiple content roles. The role manifest
defines what an artifact is; the workspace manifest defines which authored
projects belong to this source root.

The basic template creates:

- **Engine**: Rust code that provides the runtime implementation.
- **Game**: Rust code and authored content that runs on an Engine.
- **Mod**: an extension of a compatible Game or Engine, with declared
  dependencies and compatibility.
- **Packagepack**: the playable composition root that selects the Engine,
  Game, and optional Mods. It is content metadata, not a Cargo package.

Engine and Game projects typically contribute crates to the Cargo workspace.
A Packagepack describes composition and can be built and shipped without being
compiled as Rust itself. A mod can include code or data according to its
declared runtime shape.

Start with the generated example rather than inventing a manifest structure:

```text
source init basic-content PATH --organization ORG --name NAME
content list
content validate
```

For each project, inspect the role manifest and set its stable ID, version,
human-facing title and description, compatibility, and dependencies. For code
projects, keep Cargo package identity separate from Vapor content identity:
the crate is a build unit, while the Vapor ID is the distributable artifact
identity.

Validation should be run after every change to identity or dependency fields.
Before shipping, confirm the complete composition works from the packagepack
and includes every required runtime output. The
[Packagepack guide](packs-and-packagepacks.md) explains how those references
form a launchable composition.
