# SDK Core Cleanup

Status: needed.

## Exists Now

- `root/` and `steam/` are split into explicit module trees.
- `workspace/manage.rs` handles status, sync, manifest reading, file generation, and file ownership checks.
- `workspace/deploy.rs` handles build, binary promotion, aliases, and activation script generation.

## Needed Next

- Split `workspace/manage.rs` into smaller behavior-equivalent modules.
- Split `workspace/deploy.rs` into smaller behavior-equivalent modules.
- Add focused tests around file ownership and generated output before larger behavior changes.

## Total Target

Workspace code has small modules with clear file-generation, command-running, and deployment responsibilities.
