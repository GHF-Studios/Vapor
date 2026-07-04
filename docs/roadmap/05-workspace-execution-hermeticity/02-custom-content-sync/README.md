# Custom-Content Sync

Status: partially implemented.

## Exists Now

- `workspace sync` works on workspaces with kind `custom-content`.
- It reads the root `Vapor.toml` content graph.
- It generates managed Cargo workspace files.
- It generates per-content `Cargo.toml` and `Vapor.toml` files.
- It seeds `src/lib.rs` once and then treats it as user-owned.
- `Vapor-Examples` proves engine, game, and packagepack entries.

## Needed Next

- Decide prompt behavior for sync.
- Decide how much validation happens before generating files.
- Add better reports for skipped user-owned files.

## Total Target

Authors edit `Vapor.toml`; SDK sync creates or repairs the Rust workspace shell without overwriting user-owned source.
