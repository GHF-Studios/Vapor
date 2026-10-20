# Troubleshooting

## No active source workspace

Run source status. If none is open, use source open with an absolute workspace
path. The source must be outside the installed app root and contain a valid
workspace manifest.

## Project is not discovered

Check that the child is registered in Workspace.vapor.toml and has the
expected role manifest. Cargo membership and Vapor content registration are
separate; a Cargo crate alone is not necessarily a content artifact. Use
content list and content validate to see the specific error.

## Managed Rust or target is unavailable

Use Vapor's developer setup/repair workflow. Do not fall back to system Cargo:
it can select a different compiler and write unmanaged outputs. A missing
cross-linker is a separate requirement from the Rust target itself.

## Deploy succeeds but Play fails

Inspect the selected Packagepack and verify installed content. Confirm that
runtime binaries and libraries were staged for the host target. Host-only
deployment cannot be copied to another operating system and expected to work.

## Workshop create/update fails

Run content validate and a dry-run first. Check that dependencies have
PublishedFileIds, Steam is signed into the intended account, the account may
publish to the item, and the workspace declares the full release target
matrix. Use source repair to preview metadata repairs; apply them only after
reviewing the output.

## Source sync appears to do nothing

That is current behavior: source sync reports status but does not run Git. Use
your normal source-control workflow.
