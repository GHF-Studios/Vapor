# Workspace And Content Manifests

Vapor uses ordinary TOML manifests with separate owners for workspace
membership, content identity, and Rust build membership.

Workspace.vapor.toml describes a source workspace: identity, registered
content projects, runtime target matrix, and workspace-level settings. Each
registered child has a relative path. Vapor does not infer membership by
recursively finding every file with a familiar name.

A child role manifest such as Engine.vapor.toml, Game.vapor.toml, or
Packagepack.vapor.toml owns that artifact's stable ID, kind, version,
dependencies, compatibility, and publication metadata. Cargo.toml separately
defines Rust packages and workspace membership. Metadata-only Packagepacks
need not be Cargo packages.

When editing a manifest:

1. Preserve the role's identity and match it with the manifest filename.
2. Keep dependency IDs and version constraints explicit.
3. Add a Workshop PublishedFileId only after the artifact has been created.
4. Keep runtime target declarations consistent with the outputs you stage.
5. Run content validate after changing identities, dependencies, or composition.

For authoritative schemas and examples, see the Shell manifest guide. Creation
and adoption should use Vapor's guarded authoring operations so workspace and
Cargo registration do not drift.
