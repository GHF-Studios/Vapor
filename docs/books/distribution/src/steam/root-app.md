# Root App

The root app is Vapor's Steam application and launcher/runtime installation.
Its source manifest declares the Steam AppID, development branch, depot IDs,
runtime target matrix, and explicit file membership. The packaged runtime
contains launch entrypoints, binaries, resources, installed docs, and the
depot-owned default Loo-Cast App runtime. It does not contain source checkouts,
Cargo caches, SteamCMD credentials, generated state, or a mutable Workshop
Library. The default App's content packages must be prepared for the release
target matrix before root staging; missing runtime inputs fail the package.

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
