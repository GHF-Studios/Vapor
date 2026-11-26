# Player and Developer Installation Roles

## Goal

Make the local Vapor installation role a simple two-choice contract. Player is
for running and managing installed content. Developer always includes the full
Vapor-managed development toolset; it cannot be selected without that setup.
Server-side ecosystem authorization remains separate from local role state.

## Decisions

- The only local roles are `player` and `developer`.
- Developer setup is mandatory when selecting or promoting to Developer. There
  is no skip checkbox or partial Developer role.
- Pre-alpha state uses only `player` and `developer`; old role names are not
  migrated or accepted.
- Demotion to Player changes the declared role but keeps installed tools and
  caches, matching the existing non-destructive demotion contract.
- Developer owns Rust, Rust Analyzer, Linux and Windows Rust targets, Tracy,
  Zig for Linux cross-linking, and LLVM-MinGW for Windows cross-linking under
  Vapor-managed storage.
- Workflow tiers and server-side ecosystem permissions are distinct concepts;
  they are not local installation roles.

## Implementation sequence

1. Replace local role variants with Player/Developer. Role state accepts only
   these two pre-alpha identifiers; obsolete role identifiers are rejected.
2. Extend developer-environment status/reconciliation and protocol snapshots
   to cover both Rust targets and both cross-linker toolsets.
3. Store compiler/linker tools with Vapor's managed user data, not in a game
   or app-specific root; make cross-target commands consume those same paths.
4. Update Installer selection, readiness, descriptions, and transition gating
   so Developer is an all-or-nothing setup choice.
5. Update developer environment and CLI documentation; keep workflow-tier
   terminology only where it classifies action access.

## Acceptance

- The role selector exposes exactly Player and Developer.
- Selecting Developer runs complete reconciliation before persisting the role;
  no UI or command can skip the tool setup.
- Persisted role state accepts only the `player` and `developer` identifiers.
- Developer readiness is false if any declared target, linker, Rust component,
  or Tracy component is absent.
- Linux and Windows cross-build configuration uses the managed paths and does
  not depend on an app-root bootstrap script.
