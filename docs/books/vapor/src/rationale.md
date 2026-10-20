# Rationale

Vapor keeps several concepts separate because each one changes at a different
time and has a different owner. Source is edited by an author. A release is
built from a particular source version. A provider distributes releases. A
composition records the player's selection. An installation is the verified
local realization of that selection. Treating them as one “mod folder” makes
it difficult to update one composition without changing another, to identify
the bytes that were reviewed for publication, or to repair a damaged install.

## Stable identity, explicit versions

An ID answers “which authored thing is this?” A semantic version answers
“which release of it?” A dependency requirement describes the releases that a
parent can use. The resolver chooses concrete versions and records the exact
graph. This makes updates inspectable and keeps a newly discovered release
from silently changing an existing launch.

## Rust defines capability; metadata composes it

The engine and shared Rust crates define the kinds of content, valid role
relationships, required choice slots, and operations the host can perform.
Manifests and authored content select from that supported vocabulary. This
keeps arbitrary metadata from granting capabilities the engine does not
implement, while allowing authors to compose supported pieces without writing
Rust for every new content item.

## One operation owner, multiple surfaces

The GUI, CLI, Steam entrypoint, and future authoring surfaces should not each
reimplement content resolution or write local install state. The local Client
service owns operations and their results; front ends translate user intent to
typed actions and present progress or failures. This centralizes safety checks
and gives automated tools and people the same observable lifecycle.

## Providers stay replaceable

Steam Workshop is a useful distribution provider, not the definition of a Vapor
release. Vapor retains content identity, dependency semantics, and local
verification. That separation allows local source, first-party content, and
future distribution providers to participate without pretending that their
storage or permissions are identical.

Explain why Vapor is split this way.

