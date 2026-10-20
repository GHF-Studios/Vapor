# Fork A Local Pack

Forking makes an editable source copy of acquired content. It is not a second
installed copy and does not overwrite the original artifact.

1. Acquire the Workshop content into the Library and install it if needed.
2. Open the workspace where the new source project should live.
3. In Studio's creator workflow, choose the acquired artifact and use **Fork**.
   Supply a new local name. Vapor creates a new content identity and registers
   the project in the active workspace.
4. Inspect the generated role manifest and source before editing:

   ```text
   source status
   content list
   content validate
   ```

Forking requires a complete source-backed artifact. A binary-only Workshop
package cannot be reconstructed into editable Rust source. The fork is a new
authored project; it does not inherit ownership of the original Workshop item.
