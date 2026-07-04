# Steam Workshop, Registry, Trust, And Install Ledger Roadmap

Status: working roadmap.

Goal:
Separate root SteamPipe publishing from public content distribution and keep trust claims honest.

## Map

1. [Root Steam vs Workshop](01-root-steam-vs-workshop/README.md)
2. [Registry authority](02-registry-authority/README.md)
3. [Trust and install ledger](03-trust-and-install-ledger/README.md)

## Exists Now

- The SDK has SteamCMD status/login/root publish work.
- No Workshop implementation exists.
- `Vapor-Registry` exists but does not currently gate root publishing.

## Needed Next

- Which repo owns first Workshop upload, install, and launch behavior.
- Where fingerprints live before registry enforcement exists.
- Whether registry checks start as advisory diagnostics or hard blockers.
- Where install ledgers and receipts belong.

## Total Target

Root app release, public content release, registry authority, and local install state are separate workflows that can still refer to each other.
