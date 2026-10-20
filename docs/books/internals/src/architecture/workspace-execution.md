# Workspace Execution

Vapor execution starts from an explicit installation and source context. The
installed app root is discovered from the executable; the source root is
opened or selected independently. A root source may contain routed Cargo
workspaces; a content workspace has a Cargo root and explicitly registered
Vapor projects.

A managed operation should follow this path:

1. The frontend captures user intent and an explicit project/content target.
2. Typed validation resolves the app root, source root, and required tools.
3. Vapor Client owns the operation lifecycle and provider invocation.
4. Progress and terminal results return as protocol events.
5. The frontend presents the result without reconstructing domain state.

Long-running builds and uploads must not block a GUI frame loop. Cancellation,
process exit, bounded output, and operation failure should remain observable.
Never use a current working directory as a substitute for a validated source
root or let a frontend silently widen its file access.
