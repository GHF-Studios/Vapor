# Create A Local Mod

A Mod extends a compatible Engine or Game and declares that relationship in its
role manifest. The basic CLI template creates an Engine, Game, and Packagepack
together; it does not create a Mod-only workspace. Use Studio's creator
workflow to add a Mod project to an open source workspace, or adopt an
existing role manifest into that workspace.

Before adding one, open the target workspace and inspect its registered
projects:

```text
source open /path/to/my-content
content list
```

Give the Mod its own stable content ID and version. Declare the exact parent
content dependency and any additional required capabilities; do not rely on
folder names to imply compatibility. Confirm its project is registered in
`Workspace.vapor.toml`, then run:

```text
content validate
```

For a playable proof, add the Mod to the Packagepack composition and deploy the
Packagepack with `content deploy ... --select --host-only`. Keep a copy of the
workspace under version control before changing identity or dependency
metadata. See the SDK book's content-authoring guide for the role boundary.
