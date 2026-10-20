# Create A Basic Game

A Game is authored as part of a source workspace and runs on a compatible
Engine. Start with the supported combined template, then make the Game project
your code focus.

```text
source init basic-content ~/Vapor/Projects/orbit-game --organization my-studio --name orbit-game
content list
```

Open the generated Game folder and read its `Game.vapor.toml` before changing
identity or dependency fields. Add gameplay code to the Game's registered Rust
package; do not put runtime behavior in the Packagepack manifest. Keep the
generated Engine and Packagepack in place while iterating so the Game has a
composition to run in.

Validate after changing the Game identity, compatibility, or dependencies:

```text
content validate
content deploy my-studio/orbit-game/orbit-game-packagepack --select --host-only
```

If the Engine or Game declares runtime outputs, confirm the Packagepack launch
contains those files. For a shareable release, declare the complete runtime
target matrix and use the Workshop package workflow; a host-only deploy is a
development check, not a cross-platform release.
