# Content Types

Status: partially implemented in `vapor_core`.

## Exists Now

- `vapor_core` defines content types: packagepack, enginepack, gamepack, modpack, engine, game, engine mod, game mod, extension mod.
- `vapor_core` defines content sources: discovered, local, git, workshop, all.
- SDK and Launcher CLIs expose commands around those content types.

## Needed Next

- Decide which command surfaces become real first.
- Connect content identities to manifest validation and filesystem layout.

## Total Target

Content type names are stable enough for manifests, CLIs, composition, validation, and launch workflows.
