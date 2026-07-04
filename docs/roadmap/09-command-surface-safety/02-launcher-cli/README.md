# Launcher CLI

Status: mostly stub.

## Exists Now

- Launcher has core and CLI crates.
- It has typed commands for status, repair, packagepacks, packs, leaf content, lock, and launch.
- The CLI currently prints placeholder output through command specs.

## Needed Next

- Replace placeholder output with factual `not_implemented` style output.
- Decide first real Launcher slice.
- Keep install/lock/launch concepts separate from SDK authoring/publish concepts.

## Total Target

Launcher installs, selects, validates, locks, composes, and launches content for players and modpack authors.
