# Loo Cast Packagepack Steam depot

Steam distributes the Loo Cast Packagepack as a product mount. It does not
install or update Vapor. The common depot carries `Packagepack.vapor.toml`;
each supported OS depot carries one primary Packagepack executable entry
point and only the runtime sidecars explicitly required by that target. A DLL
beside the executable does not create a second product entry point.

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
