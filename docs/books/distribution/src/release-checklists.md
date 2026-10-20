# Release Checklists

## Vapor root app

- Confirm the source branch and root manifest are current.
- Run the managed validation workflow.
- Confirm the complete Linux and Windows runtime matrix and required native
  support files.
- Run root publish dry-run and inspect every staged depot and generated VDF.
- Confirm the dedicated build account and non-default Steam branch.
- Perform the explicitly confirmed upload, then verify Steam's build state.

## Workshop content

- Validate the registered source workspace and all dependencies.
- Confirm stable IDs, versions, compatibility, PublishedFileIds, and visibility.
- Build the full declared runtime matrix.
- Review the actual staged files, target directories, and dependency graph.
- Dry-run creation or update; ensure the selected Steam account owns the item.
- Publish only after explicit confirmation.
- Refresh the catalog and verify the resulting item and local installation.

A local host-only deploy proves only a development path. It is not evidence that
a release contains other platforms or that Steam accepted the upload.
