# SteamPipe

SteamPipe distributes the Loo Cast Packagepack product. Its app-source
manifest owns the AppID, development branch, depot IDs, runtime targets, and
allowlisted source-to-depot includes. Steam build VDF templates are filled with
the staged paths and release settings by Vapor. Vapor itself is distributed
independently of Steam.

Use the preview before any upload:

```text
root publish --dry-run
```

This stages the payload and writes preview VDFs without contacting SteamCMD.
Inspect the common and platform depot contents. The common depot carries
`Packagepack.vapor.toml`; Linux and Windows depots each carry one primary
Packagepack executable plus only explicitly declared runtime sidecars. No
Vapor runtime or mutable Vapor state is included.

A real publication always uses the full declared Linux and Windows runtime
matrix. The local host-only and custom-target options are preview paths, not
release shortcuts. The upload is an explicit action because it changes the
Steam app branch. Packagepack publication does not publish Workshop content.
