# Packs And Packagepacks

Vapor has typed pack roles for grouping compatible content and Packagepack for
the playable root. Allowed child slots are defined by Rust schemas; they are
not arbitrary script-defined names.

A Packagepack selects the Engine and Game that form the playable program and
can include compatible packs or mods. A pack groups related content for a
supported slot. The Packagepack itself is metadata-driven: staging it resolves
its child graph and runtime artifacts without compiling the Packagepack
manifest as Rust.

When composing:

- Use stable content IDs, not folder names.
- Declare compatibility and version constraints on dependencies.
- Keep optional content optional in the manifest and resolved graph.
- Check for duplicate identities and conflicts before deploying.
- Preserve the distinction between source composition and a player's
  installed Composition.

Create and validate the generated example:

```text
source init basic-content PATH --organization ORG --name NAME
content validate
content deploy ORG/NAME/NAME-packagepack --select --host-only
```

The Shell manifest guide has exact fields; the Vapor Book explains the
difference between a Packagepack artifact and a player's Composition.
