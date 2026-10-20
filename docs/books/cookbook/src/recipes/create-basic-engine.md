# Create A Basic Engine

The supported starter is a workspace template, not a standalone Engine wizard.
It creates an Engine, Game, and Packagepack together so their identities and
references have a valid starting shape.

1. Create the workspace in an empty location:

   ```text
   source init basic-content ~/Vapor/Projects/stellar-demo --organization my-studio --name stellar-demo
   ```

2. Inspect the generated Engine project and its `Engine.vapor.toml`. Add Rust
   engine code in the registered Cargo project; keep the content ID and crate
   name conceptually separate.
3. Validate the graph and build the registered Cargo workspaces:

   ```text
   content validate
   build --project all
   ```

4. Deploy the Packagepack to exercise the Engine in its composition:

   ```text
   content deploy my-studio/stellar-demo/stellar-demo-packagepack --select --host-only
   ```

Host-only deployment proves only the current machine's runtime. See
[Content Authoring](../../sdk/workflows/content-authoring.html) for the
manifest roles and [Quick Start](../../sdk/quick-start.html) for the complete
first-run sequence.
