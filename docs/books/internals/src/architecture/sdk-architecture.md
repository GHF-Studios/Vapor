# SDK Architecture

The SDK is a set of managed authoring operations built on shared Vapor
semantics. Cargo remains authoritative for Rust package membership and build
behavior. Workspace manifests register Vapor content projects; role manifests
define identity, version, dependencies, compatibility, and publication fields.

The managed toolchain owns Rust and Cargo selection, target installation,
environment, and output roots. SDK commands request operations through Vapor
rather than reconstructing compiler policy in each command. A content package
is staged from the declared runtime outputs and a resolved role manifest.

Do not conflate:
- Rust crate with Vapor content identity;
- Cargo workspace member with registered Vapor project;
- source composition with installed Composition;
- build output with deployable artifact;
- local package with Workshop publication.

Each boundary should be observable in command results and documented where
authors make the corresponding decision.
