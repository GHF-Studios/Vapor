# Repository Topology Roadmap

Status: working roadmap.

Goal:
Make the active repo split explicit enough that code, docs, packaging, and release work stop drifting.

## Map

1. [Active repos](01-active-repos/README.md)
2. [Future repos](02-future-repos/README.md)

## Exists Now

- `Vapor`
- `Vapor-SDK`
- `Vapor-Launcher`
- `Vapor-Examples`
- `Vapor-Registry`

## Needed Next

- Decide whether `Vapor-Registry` is active authority now or active context only.
- Decide whether the repo stack wording in `AGENT.md` needs revision.
- Keep Spacetime/Loo Cast content out unless active Vapor context requires it.

## Total Target

- Each repo owns a clear part of the ecosystem.
- Development dependencies and release dependencies are handled deliberately.
- Future Spacetime/Loo Cast repos do not leak concrete game/engine internals into Vapor.
