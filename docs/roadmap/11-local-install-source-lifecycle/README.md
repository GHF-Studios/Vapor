# Local Installation And Source Lifecycle

This ledger captures the clean-machine workflow that must work before Vapor infrastructure is considered reconstructable.

## Ownership domains

1. **Steam App Instance** — replaceable product state: installed binaries, bootstrap executables, shipped metadata and resources.
2. **Vapor User Data** — persistent mutable/derived state: role, source configuration, managed toolchains, build outputs, deployment staging, IDE/provider state and caches.
3. **Canonical Superworkspace** — authored Git/source, explicitly configured and preserved independently of ordinary App uninstall/reinstall.

## Normal bootstrap

```text
Install Loo Cast through Steam
vapor-installer install
vapor-installer role promote ecosystem-developer
vapor source setup [SUPERWORKSPACE]
vapor source acquire first-party-all
```

`installation diagnose` and `installation repair` are recovery operations, not onboarding steps.

## Source lifecycle

```text
vapor source setup [SUPERWORKSPACE]

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

The canonical Superworkspace is represented by `Superworkspace.vapor.toml` and persisted in Vapor User Data. An empty configured Superworkspace is therefore discoverable before any source is acquired.

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
- [ ] Establish `vapor-installer install` as the normal base bootstrap.
- [ ] Persist and create a canonical marker-backed Superworkspace.
- [ ] Implement granular first-party acquisition.
- [ ] Implement safety-gated granular source removal and teardown.
- [ ] Restore host-target-specific installed/staged binary layout.
- [ ] Register `Loo-Cast`, `Vapor-Client`, and `Vapor-Platform-Server` as canonical first-party acquisition roots.
- [ ] Restore Loo Cast's current-schema `Workspace.vapor.toml`.
- [ ] Normalize special root manifests to `Vapor-Client.vapor.toml` and `Vapor-Platform-Server.vapor.toml`.
- [ ] Build/test/deploy locally and push every accepted repository change.
- [ ] Update/deploy the Platform Server Registry submodule.
- [ ] Audit the fresh Steam payload and Steamworks Launch Options.
- [ ] Run the destructive Steam uninstall/reinstall/reconstruct gauntlet.
