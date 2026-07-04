# Lockfiles And Receipts

Status: not implemented.

## Exists Now

- Code mentions future packagepack locks and generated state.
- No `Vapor.lock` implementation exists.
- Root package/publish reports exist in memory/CLI output, not as durable receipts.

## Needed Next

- Pick the first durable generated artifact.
- Decide whether that is a toolchain receipt, root publish receipt, packagepack lock, install ledger, or something smaller.
- Keep it separate from human-authored `Vapor.toml`.

## Total Target

Generated state records what was resolved, installed, built, packaged, published, or selected.
