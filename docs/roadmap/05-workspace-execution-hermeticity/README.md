# Workspace Execution And Hermeticity Roadmap

Status: working roadmap.

Goal:
Define what SDK-managed workspace commands actually guarantee.

## Map

1. [Managed Cargo](01-managed-cargo/README.md)
2. [Custom-content sync](02-custom-content-sync/README.md)
3. [Deploy](03-deploy/README.md)

## Exists Now

- Vapor-managed Cargo commands write output under `$VAPOR_HOME/output/dev/<workspace>`.
- Source repo `target/` directories are not the intended output location for Vapor-managed builds.
- `RUSTC_WRAPPER` is removed in current workspace command handling.
- Existing PATH is still preserved after Vapor path prefixes.

## Needed Next

- Whether preserving PATH is compatible with "no system leaks" language.
- Which environment variables must be scrubbed before any hermeticity claim.
- Whether `workspace sync`, `sdk fmt`, and `workspace deploy` need different safety prompts.
- Whether `workspace deploy` is public SDK workflow or internal root maintenance.

## Total Target

SDK workspace commands should be predictable about what they read, what they write, and which tools they invoke.
