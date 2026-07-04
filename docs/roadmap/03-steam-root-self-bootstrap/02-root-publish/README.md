# Root Publish

Status: partially implemented in SDK.

## Exists Now

- `sdk root publish` packages the root, then calls SteamCMD with `+login` and `+run_app_build`.
- `--plan` can build the package/publish plan without invoking SteamCMD.
- SteamCMD can be provided by path or resolved from PATH.
- SteamCMD owns password prompts, Steam Guard, and saved login tokens.
- Default branch activation stays manual; `--set-live` is for beta branches.

## Needed Next

- Decide whether registry checks are advisory or blocking.
- Decide where SteamCMD state must live.
- Decide whether SteamCMD acquisition belongs in the SDK.

## Total Target

Root publishing is a controlled SteamPipe workflow for the first-party app root, separate from future Workshop content publishing.
