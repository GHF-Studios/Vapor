# Packs, Capability, Rhai, And Runtime Concepts Roadmap

Status: working roadmap.

Goal:
Keep runtime vocabulary from swallowing the bootstrap.

## Map

1. [Content types](01-content-types/README.md)
2. [Pack composition](02-pack-composition/README.md)
3. [Capability, Rhai, and runtime](03-capability-rhai-runtime/README.md)

## Exists Now

- Packagepack, Enginepack, Gamepack, and Modpack are comparatively stable vocabulary.
- `vapor_core` has content type and pack-child vocabulary.
- Capability, Rhai, runtime lock, and script-safety vocabulary need careful handling before implementation claims.
- Rhai validation is not the immediate active work.

## Needed Next

- The first pack validation primitive.
- When reserved names such as `core_engine`, `core_mod`, and `base_mod` become enforced.
- The smallest capability concept worth implementing in `vapor_core`.
- Whether early Rhai work should be file classification before parsing, linting, or execution.

## Total Target

Packs and runtime concepts become code only when they can be validated or launched by a real tool path.
