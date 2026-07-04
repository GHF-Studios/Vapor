# Self-Bootstrap

Status: target not proven yet.

## Exists Now

- SDK can deploy first-party CLI binaries into an app-root-like executable root.
- SDK can package that app root for Steam.
- SDK can invoke SteamCMD to publish the package.

## Needed Next

- Run a relocation test against the Steam/app-root layout.
- Prove the Steam-downloaded SDK can check/build/deploy/package/publish the next app root.
- Decide which developer hot-deploy shortcuts are allowed only for local root development.

## Total Target

The Steam-distributed app can maintain itself through the SDK it ships.
