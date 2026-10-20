# SteamCMD

SteamCMD is Vapor's managed Steam distribution provider. It is used for
SteamPipe app uploads and some Workshop publishing operations. It is not the
normal player interface for Workshop discovery or subscription.

Vapor installs a pinned SteamCMD tool under its managed application data and
starts it through typed publishing operations. Do not copy its login state into
the depot or a source repository. Do not put passwords or Steam Guard codes in
command arguments, scripts, or manifests.

A dry-run validates Vapor's staged package but cannot prove authentication,
account ownership, beta-branch rights, or remote Steam acceptance. A real
upload asks for an account and explicit confirmation. Use the dedicated build
account for root release uploads and the appropriate publisher identity for
Workshop content.

If SteamCMD is missing or its session is unusable, use Vapor's managed setup
and follow the operation's authentication prompt. Do not treat raw SteamCMD
commands as the supported player install path.
