# Testing And Release

Evidence must match the claim. A format check proves formatting; a build proves
compilation for selected targets; tests prove covered behavior; package
validation proves the staged tree passed its checks. None alone proves a game
launches on every platform or that Steam accepted a release.

Use Vapor's managed format, check, test, build, and validate workflows for
Vapor Client work. Content changes also require content validation and a local
deployment where appropriate. A host-only deploy is explicitly a local smoke
path.

Root-app release and Workshop content publication have separate preflights.
Run the relevant dry-run and inspect target matrix, identity, staged files,
dependencies, and branch/item before a real upload. After deployment, verify
the public route or Steam build state and update deployment evidence.

Documentation needs a separate render and link check. The docs bundle must be
built and validated before upload; a successful upload should be followed by
public index and page checks. Do not report release success based only on a
local staging directory.
