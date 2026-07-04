# Manifest, Lockfile, Artifact, And Fingerprint Roadmap

Status: working roadmap.

Goal:
Keep `Vapor.toml`, future lock files, artifacts, and fingerprints small and real.

## Map

1. [`Vapor.toml`](01-vapor-toml/README.md)
2. [Lockfiles and receipts](02-lockfiles-and-receipts/README.md)
3. [Fingerprints](03-fingerprints/README.md)

## Exists Now

- Active repos contain `Vapor.toml`.
- No `Vapor.lock` implementation exists.
- No fingerprint implementation exists.

## Needed Next

- The minimal active `Vapor.toml` schema.
- The first lock-like artifact.
- The first fingerprint representation.
- Which artifact terms active code may use now.

## Total Target

Human intent stays in manifests. Generated state, resolved versions, hashes, receipts, and locks go somewhere else.
