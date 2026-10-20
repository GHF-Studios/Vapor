# SteamCMD

SteamCMD is a managed distribution tool for root-app and Workshop publishing.
It is not required for ordinary Play or local content editing. Player Workshop
discovery and subscription use Steam client APIs when available.

Vapor owns the pinned tool installation and invokes it through typed
operations. Do not install another SteamCMD build into Vapor's app root or
write credentials into a manifest. Authentication may require interactive
login, Steam Guard, or the dedicated build account.

Before publishing, run the relevant dry-run. A successful preview verifies
Vapor's validation and staging path; it does not prove Steam authentication,
ownership, branch permission, or remote acceptance. Real publishing requires
explicit account selection and confirmation.

Use Vapor's developer setup and diagnostics to resolve a missing SteamCMD.
Do not operate on Vapor's managed SteamCMD state directly unless an official
workflow explicitly asks you to.
