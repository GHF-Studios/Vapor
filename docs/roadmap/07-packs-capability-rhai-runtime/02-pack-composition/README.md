# Pack Composition

Status: early model exists.

## Exists Now

- `ChildContentRef` exists.
- `allowed_pack_children` defines first child rules:
  - packagepacks may contain every current content type.
  - enginepacks may contain engines, engine mods, and enginepacks.
  - gamepacks may contain games, game mods, and gamepacks.
  - modpacks may contain engine mods, game mods, extension mods, and modpacks.
- SDK and Launcher command models include add/remove/select/unselect composition commands.

## Needed Next

- Implement actual graph validation.
- Decide reserved names and core artifact layout.
- Decide how packagepack lock generation resolves selected children.

## Total Target

Pack composition becomes a validated graph that SDK can author and Launcher can lock/launch.
