# The Vapor Launcher Book

The Launcher is the player-facing product: discover content, subscribe or
install it, choose a playable Composition, update or repair that Composition,
and Play. Ordinary player workflows should not expose source workspaces, Cargo,
SteamCMD, build caches, manifests, or publishing controls.

The owner-approved product direction separates three experiences:

- **Launcher** for players and installed Compositions.
- **Studio** for creators assembling, validating, running, and publishing
  content.
- **IDE** for editing code and navigating source projects.

They share Vapor's local Client service and the same identities, dependency
rules, provider state, progress, and operation results. This book describes the
player journey; where current UI behavior differs from that target, the page
calls it out. Today the player area still ships inside Vapor Studio.

Start with [Quick Start](quick-start.md), then learn the difference between a
[content source](workflows/content-sources.md), the local
[Library](workflows/installed-state.md), and an installed
[Composition](workflows/packagepacks.md). Use
[Install, Update, Remove](workflows/install-update-remove.md) for lifecycle
actions and [Lock And Launch](workflows/lock-and-launch.md) for the resolved
versions actually used by the game.
