# Packagepacks

A Packagepack is a typed content artifact that roots a playable Composition.
It names the Engine and Game to use and may include compatible Packs or Mods.
The Packagepack is not the same as the player's Composition record: the
Composition also records user selection and resolved versions.

This distinction keeps authored intent separate from the installed instance:

- The Packagepack artifact is source-controlled and can be published.
- The Composition is the user's launchable selection of that artifact and its
  resolved content graph.
- The Installation is the local realization of the Composition's exact
  artifacts and runtime files.
- The Workspace is editable source and may produce several artifacts.

Packagepack is metadata, not a Rust crate. It can select and constrain child
content, but the Engine and Game remain distinct projects with their own
identities and code. Vapor validates references and compatibility before
deployment or publication.

Create and deploy a basic local example:

```text
source init basic-content PATH --organization ORG --name NAME
content validate
content deploy ORG/NAME/NAME-packagepack --select --host-only
```

A host-only deploy is a local proof. A shareable Packagepack must be staged
with the declared runtime target matrix and published as one logical artifact.
