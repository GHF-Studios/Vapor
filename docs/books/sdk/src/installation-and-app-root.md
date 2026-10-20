# Installation And App Root

Vapor's installed application root is the managed runtime used to launch games
and tools. It contains installed binaries, app-owned configuration, acquired
content, and build output. It is not the source workspace and should not be
used as a substitute for a Git checkout.

A source workspace is opened or indexed by Vapor. Its registered content and
Cargo projects remain at their authored paths. Local build products are
written to managed output locations instead of being mixed into project
folders. This separation lets the app update itself without taking ownership of
the author's source files.

The app root is discovered by Vapor from its installation and platform
context. Prefer the app's own commands and diagnostics to manually editing
internal directories. For example, `source status` reports the active source,
and `root status` reports installed root-app state. Build and deployment
commands explain their output locations.

When troubleshooting, first distinguish these cases:

- **Source problem:** a manifest, project registration, dependency, or source
  file is missing or invalid. Inspect the workspace and use `source repair`
  to preview safe metadata repairs.
- **Installed content problem:** an artifact is absent, stale, or damaged.
  Inspect content status and use the content repair workflow.
- **Application-root problem:** a required executable or tool is absent. Use
  the managed root build/deploy path rather than copying files into the app
  root by hand.

The exact on-disk layout is implementation detail for most users. The Launcher book covers player-facing install and repair actions; the
Internals book describes the current layout for maintainers.
