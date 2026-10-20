# Create A Basic Packagepack

A Packagepack is the playable composition root. It selects the Engine and Game
and may include compatible Mods. It is metadata-driven; it is not a Rust crate.

Create the supported starter workspace and validate the generated composition:

```text
source init basic-content ~/Vapor/Projects/first-flight --organization my-studio --name first-flight
content validate
content list
```

Review `Packagepack.vapor.toml`. Confirm it references the intended Engine
and Game IDs and that their versions satisfy the declared constraints. Add
optional content only when its identity and compatibility are known. Then
deploy and select the Packagepack locally:

```text
content deploy my-studio/first-flight/first-flight-packagepack --select --host-only
content selected
```

The deployment builds the needed registered source projects and stages a
playable artifact in Vapor's app-owned content area. It does not publish a
Workshop item. For the player-side install path, see
[Install A Packagepack](install-packagepack.md); for first publication, see
[Publish To Workshop](publish-to-workshop.md).
