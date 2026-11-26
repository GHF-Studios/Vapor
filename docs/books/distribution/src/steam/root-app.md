# Loo Cast Packagepack Steam depot

The root app is Vapor's Steam application and launcher/runtime installation.
Its source manifest declares the Steam AppID, development branch, depot IDs,
runtime target matrix, and explicit file membership. The packaged runtime
contains launch entrypoints, binaries, resources, installed docs, and the
depot-owned default Loo-Cast App runtime. It does not contain source checkouts,
Cargo caches, SteamCMD credentials, generated state, or a mutable Workshop
Library. The default App's content packages must be prepared for the release
target matrix before root staging; missing runtime inputs fail the package.

The Packagepack executable composes the Spacetime Engine and Loo Cast Game and
launches directly from Steam. The Steam payload excludes `App.vapor.toml`, the
Vapor runtime, Vapor state, source checkouts, tools, and loose Engine/Game
directories. The Loo Cast app source manifest is `App-Source.vapor.toml` in the
Loo Cast source checkout.

Preview the actual allowlist before uploading:

```text
root package --release-targets
root publish --dry-run --release-targets
```

The generic `root` command name is a CLI workflow label. In this checkout it
stages the declared Loo Cast Steam package; it does not mean Vapor itself is a
Steam application. Inspect the shared manifest depot and both target depots,
including all declared runtime sidecars. Real publication must use the full
Linux and Windows target matrix and an explicitly configured non-default
branch.
