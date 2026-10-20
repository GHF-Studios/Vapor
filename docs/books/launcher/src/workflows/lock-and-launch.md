# Lock And Launch

Before launching, Vapor resolves the Composition's root Packagepack to an
exact graph of content IDs, versions, and dependency edges. The installation
record also tracks provenance and payload fingerprints. This graph is the
Composition's selected runtime, not a promise to always use the latest
Workshop versions.

## What the resolved graph protects

- **Reproducibility:** the same Composition refers to the same selected
  releases until the user or an authorized Auto-Install policy updates it.
- **Integrity:** Vapor can compare installed bytes with the recorded payload
  fingerprints and identify local changes or damaged files.
- **Dependency safety:** the host can verify that required slots are present,
  content kinds are permitted, version requirements match, and the graph has
  no cycle.
- **Recovery:** if files are missing or changed, Vapor can check available
  source/cache copies before repairing the installation.

## Launch checks

The service validates the selected Composition, root Packagepack, resolved
graph, and installation before handing off to the game. A missing dependency
can be acquired if a configured provider can supply it. A conflicting local
copy, stale resolution, or failed verification must be surfaced so the player
can repair or review it rather than silently launching an arbitrary version.

The installed Shell's `launch default` command uses the app's default
playable Composition. `launch selected` uses the composition selected in
Studio. The direct Steam Play entrypoint can start the currently selected
composition without navigating through creator tools.

## Updating a lock

An update is the explicit transition to a new resolved graph. Show a concise
summary of added, removed, and changed releases, then apply and verify the new
installation. Until the operation succeeds, the old complete installation is
the recovery point. Do not edit generated lock or receipt files by hand; use
the supported update, repair, and uninstall operations.

