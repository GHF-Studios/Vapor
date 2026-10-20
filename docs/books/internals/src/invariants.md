# Invariants

These are cross-cutting rules for Vapor changes:

- Cargo owns Rust packages, targets, features, dependencies, and profiles.
  Vapor manifests own Vapor identities and content relationships.
- Workspace registration is explicit. A nested role manifest is not content
  until its project path is registered.
- Source workspaces remain separate from the installed app root and user data.
- Canonical paths and containment are checked before filesystem mutation.
- A frontend submits intent; the service validates and owns persistent state.
- Providers return owned data and explicit operation outcomes.
- Composition, source, downloaded Library content, installation, and release
  are distinct linked objects.
- App-root builds, Workshop content packages, and installed game compositions
  have different artifact and publication boundaries.
- Managed Rust/Cargo commands never fall back to an unrelated system toolchain.
- Previews do not change remote Steam authority.
- Local deployment or host-only builds do not prove another platform works.
- No user-facing success claim exceeds the evidence from build, install, launch,
  or publication.

When a change appears to violate an invariant, update the semantic owner and
every consuming frontend together; do not hide the mismatch in a presentation
adapter.
