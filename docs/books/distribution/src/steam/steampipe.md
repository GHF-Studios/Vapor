# SteamPipe

SteamPipe publishes the Vapor root application and its depots. The root
manifest owns AppID, development branch, depot IDs, runtime targets, and
allowlisted source-to-depot includes. Steam build VDF templates are filled with
the staged paths and release settings by Vapor.

Use the preview before any upload:

```text
root publish --dry-run
```

This stages the payload and writes preview VDFs without contacting SteamCMD.
Inspect the common and platform depot contents. The common depot carries
platform-neutral files; Linux and Windows depots carry their matching binaries,
support files, and launch script.

A real publication always uses the full declared runtime matrix. The local
host-only and custom-target options are preview paths, not release shortcuts.
The upload is an explicit action because it changes the Steam app branch.
Root-app publication does not publish a game's Workshop content.
