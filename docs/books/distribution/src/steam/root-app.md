# Root App

The root app is Vapor's Steam application and launcher/runtime installation.
Its source manifest declares the Steam AppID, development branch, depot IDs,
runtime target matrix, and explicit file membership. The packaged runtime
contains launch entrypoints, binaries, resources, and installed docs. It does
not contain source checkouts, Cargo caches, SteamCMD credentials, generated
state, or game Workshop payloads.

Use a local package to inspect the staged app without upload:

```text
root package --host-only
root publish --dry-run
```

The first command is a local host smoke package. The release preview validates
the root, builds the declared matrix, stages the depots, and produces SteamPipe
VDF previews. A real upload requires an explicit account and confirmation on a
non-default branch.

The current manifest's stable entrypoints include Studio, Play, Shell, and
Installer. The owner-approved Launcher/Studio/IDE product split is newer than
that packaging model; audit the root manifest and installed entrypoints before
changing the mapping.
