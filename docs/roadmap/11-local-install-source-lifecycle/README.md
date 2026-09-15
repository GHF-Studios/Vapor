# Local Installation And Source Lifecycle

This ledger captures the clean-machine workflow that must work before Vapor infrastructure is considered reconstructable.

## Ownership domains

1. **Steam App Instance** — replaceable product state: installed binaries, bootstrap executables, shipped metadata and resources.
2. **Vapor User Data** — persistent mutable/derived state: role, source configuration, managed toolchains, build outputs, deployment staging, IDE/provider state and caches.
3. **Canonical Superworkspace** — authored Git/source, created automatically at the single canonical user root and preserved independently of ordinary App uninstall/reinstall.

## Normal bootstrap

```text
Install Loo Cast through Steam
vapor-installer install
vapor-installer role promote ecosystem-developer
vapor source acquire first-party-all
```

`installation diagnose` and `installation repair` are recovery operations, not onboarding steps.

## Source lifecycle

```text
vapor source setup

vapor source acquire loo-cast
vapor source acquire vapor-client
vapor source acquire vapor-platform-server
vapor source acquire vapor-all
vapor source acquire first-party-all

vapor source remove loo-cast [--yes]
vapor source remove vapor-client [--yes]
vapor source remove vapor-platform-server [--yes]
vapor source remove vapor-all [--yes]
vapor source remove first-party-all [--yes]

vapor source teardown [--yes]
```

The single canonical Superworkspace is represented by `Superworkspace.vapor.toml`, persisted in Vapor User Data, and created automatically by `vapor-installer install`. `vapor source setup` is an idempotent create/repair operation rather than first-run configuration. Successful source acquisition also materializes/reconciles the managed RustRover development environment, so ordinary onboarding does not require a follow-up `vapor installation repair`.

`--yes` means **confirmation only**. It never bypasses dirty/unpublished Git safety checks. Group removal validates every selected checkout before deleting any checkout.

## First-party selectors

- `loo-cast` → `Loo-Cast`
- `vapor-client` → `Vapor-Client`
- `vapor-platform-server` → `Vapor-Platform-Server`
- `vapor-all` → both Vapor operational roots
- `first-party-all` → all three

Registry data remains authoritative for acquisition URLs and checkout paths. Loo Cast is an ordinary content Workspace and therefore uses `Workspace.vapor.toml`; special operational/topology manifests stay in the same Vapor/TOML family but use repository-specific names where useful, including `Vapor-Client.vapor.toml` and `Vapor-Platform-Server.vapor.toml`.

## Installed binary layout

```text
<installation>/bin/<host-target>/vapor
<installation>/bin/<host-target>/vapor-installer
```

Local deployment, Steam staging, activation and machine integration must use the same host-target directory.

## Uninstall

- `vapor-installer uninstall` removes machine integration and obsolete App-local mutable directories.
- `--purge-app-external` additionally deletes Vapor User Data.
- `--purge-superworkspace` independently deletes the canonical authored-source root.
- Destructive purge flags require confirmation unless `--yes` is supplied.

## Remaining clean-machine limitation

Composer-or-higher Role promotion still requires Git on `PATH`; Vapor does not yet provision Git. Until that is fixed or explicitly declared a prerequisite, a truly blank machine is not fully self-hosting.

## Execution ledger

- [x] Move mutable Rust/Cargo/build/Steam staging state outside the Steam App Instance.
- [x] Preserve ordinary uninstall vs explicit purge semantics.
- [x] Establish `vapor-installer install` as the normal base bootstrap.
- [x] Persist and automatically create the single canonical marker-backed Superworkspace.
- [x] Implement granular first-party acquisition.
- [x] Implement safety-gated granular source removal and teardown.
- [x] Restore host-target-specific installed/staged binary layout.
- [x] Register `Loo-Cast`, `Vapor-Client`, and `Vapor-Platform-Server` as canonical first-party acquisition roots.
- [x] Restore Loo Cast's current-schema `Workspace.vapor.toml`.
- [x] Normalize special root manifests to `Vapor-Client.vapor.toml` and `Vapor-Platform-Server.vapor.toml`.
- [x] Make source acquisition finalize managed RustRover state and generated workflow configurations.
- [ ] Build/test/deploy locally and push every accepted repository change.
- [ ] Update/deploy the Platform Server Registry submodule.
- [ ] Audit the fresh Steam payload and Steamworks Launch Options.
- [ ] Run the destructive Steam uninstall/reinstall/reconstruct gauntlet.
