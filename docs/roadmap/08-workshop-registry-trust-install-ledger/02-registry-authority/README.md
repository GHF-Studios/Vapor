# Registry Authority

Status: repo exists; enforcement not implemented.

## Exists Now

- `Vapor-Registry` has workspace kind `registry`.
- Its manifest names GitHub as the authority location.
- Its README describes identity, Steam app metadata, and permission grants.
- SDK root publish does not currently consult the registry.

## Needed Next

- Decide advisory diagnostics vs hard blocking.
- Decide how GitHub identity and Steam identity bind.
- Decide which registry files become machine-readable authority.

## Total Target

Registry checks give Vapor a public, reviewed authority before release actions, while Steam still enforces final Steamworks permissions.
