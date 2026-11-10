# Release Checklists

## Loo Cast Packagepack Steam depot

- Confirm the Loo Cast source branch and `App-Source.vapor.toml` are current.
- Run the managed validation workflow.
- Build the Packagepack for the full Linux and Windows target matrix.
- Run the Steam depot dry-run and inspect `Packagepack.vapor.toml`, each
  target's primary executable, and every declared runtime sidecar.
- Confirm no Vapor runtime, `App.vapor.toml`, source, tool, or state files are
  staged.
- Confirm the dedicated build account and non-default Steam branch.
- Perform the explicitly confirmed upload, then verify Steam's build state.

Vapor runtime releases use Vapor's independent release service and do not use
this Steam checklist.

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
