# Deploy

Status: partially implemented for first-party tools.

## Exists Now

- `sdk deploy` builds `vapor_sdk_cli` in SDK workspaces.
- `sdk deploy` builds `vapor_launcher_cli` in Launcher workspaces.
- It promotes binaries into app-root `bin/`.
- It writes `sdk_cli` or `launcher_cli` aliases.
- It writes an activation script.

## Needed Next

- Decide whether deploy is public SDK behavior or internal root maintenance.
- Decide whether hot-deploy into Steam installs is explicitly developer-only.
- Decide whether Launcher placeholder state blocks deployment or root packaging.

## Total Target

Deploy is a controlled way to put first-party tool binaries into an app root, with clear safety boundaries.
