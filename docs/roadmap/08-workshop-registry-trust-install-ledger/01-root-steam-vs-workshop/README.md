# Root Steam Vs Workshop

Status: root Steam exists; Workshop not implemented.

## Exists Now

- SDK root publish uses SteamCMD/SteamPipe for AppID `2122620`.
- Steam Workshop commands are not implemented.

## Needed Next

- Keep root package/publish command names separate from future Workshop publish/install command names.
- Decide whether SDK owns first Workshop upload and Launcher owns first Workshop install/launch.

## Total Target

SteamPipe updates the first-party app root. Workshop distributes content. The tools should not blur those two jobs.
