# The Vapor Internals Book

This book helps contributors find code owners and preserve boundaries while
changing Vapor. Start with the repository map, then follow the component whose
responsibility matches the change. The implementation spans the shared Vapor
crates, Vapor-Client, Vapor-CLI, Vapor-GUI, Vapor-Studio, Steam entrypoints,
and the Platform Server repositories.

The system is pre-alpha and public seams may change, but changes should
strengthen the shared semantic core rather than create another catalog,
resolver, lifecycle, or toolchain path. Product UI boundaries are evolving:
Launcher, Studio, and IDE are separate approved experiences over one local
Vapor Client service.

Read [Invariants](invariants.md) before touching persistence or filesystem
operations. Read [Testing And Release](architecture/testing-and-release.md)
before validating or shipping a change.
