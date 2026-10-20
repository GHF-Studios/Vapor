# What Vapor Is

Vapor is a content and application lifecycle system for games built around
versioned, typed packages. It connects authoring workspaces to validated
releases, combines compatible releases into a playable composition, and
installs that composition through controlled providers such as Steam Workshop.

Vapor is also a family of frontends built on shared Rust code. The owner-
approved direction separates Launcher for players, Studio for creators, and
IDE for source editing, alongside CLI/Shell for support and recovery. The
Vapor Client service owns stateful operations. These frontends must agree
because they ask the same service to do the work. Adding an interface must not
create another content catalog, version resolver, or installation rule set.

The current GUI still bundles player and creator workflows in Vapor Studio;
Launcher and IDE are approved product boundaries, not yet separate shipped
applications.

## From source to a playable setup

An author works in a source workspace. The workspace manifest explicitly
registers its projects; a registered content manifest defines each project's
Vapor kind, identity, version, dependencies, and role-specific metadata. Cargo
builds code-bearing projects through Vapor's pinned toolchain. Packaging then
copies declared runtime outputs into a versioned artifact and records the
payload that was actually staged.

A player discovers or receives releases from an enabled source, downloads a
release into the local Library, and chooses it for a composition when it fits
the required role. Vapor resolves the complete dependency graph, records the
selected versions and provenance, installs the files, verifies them, and hands
the resulting launch configuration to the game. Repair uses the same recorded
identity and fingerprints to detect drift.

Each step has a different input and result. A source workspace is not a
Workshop download; a download is not an installation; an installed artifact is
not the composition that refers to it. Keeping those boundaries explicit makes
updates, forks, recovery, and multiple compositions possible without copying
every file into every profile.

## What makes content usable

Content kinds are defined by Vapor's Rust crates and shared by every front
end. Authors choose from the supported vocabulary; templates help create
those kinds but do not invent new kinds. The manifest records dependency
requirements and relationships. The host checks that a graph is valid, that
required choice slots are satisfied, and that selected runtime targets are
available before staging a release.

Steam Workshop can provide discovery, subscriptions, and downloaded item
payloads. Vapor treats it as one provider, not as the entire semantic model:
the local Library, composition graph, verification records, and source
workspaces remain explicit Vapor state. Local and first-party content can be
authored and inspected without Steam, while Steam-backed operations still
require its client and account.

## Where to go next

Read [What Vapor Is Not](what-vapor-is-not.md) for current boundaries and
limits. Read [Content Types](concepts/content-types.md) for every canonical
kind, then [Packs And Composition](concepts/packs-and-composition.md) for how
those kinds form a playable graph.

