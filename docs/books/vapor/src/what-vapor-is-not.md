# What Vapor Is Not

Vapor is not just a folder convention or a Workshop downloader. Its manifests,
local Library, compositions, and installation records have distinct meanings
and are checked by shared code. A file placed under an arbitrary directory is
not automatically a Vapor content item; a release is not a source checkout; a
Steam subscription is not by itself proof that a composition is installed and
ready to launch.

Vapor is also not a promise that every workflow is finished. Some documentation
books and authoring paths are still being completed. Steam-backed search,
subscription, download, and upload require Steam's client and permissions.
Cross-platform releases require the declared platform matrix to build
successfully. A successful local build does not prove that the Workshop item is
correctly configured or that a different operating system can launch it.

## Boundaries to keep in mind

- **Steam is a provider, not Vapor's entire data model.** Steam stores
  subscriptions and Workshop payloads. Vapor separately tracks local content,
  versions, compositions, installations, and verification state.
- **The Library is not the active composition.** A downloaded item can be
  available without being selected or installed into the composition you play.
- **A composition is not a Packagepack.** The Packagepack is its authored root;
  the composition also records the user's selected root, resolved graph,
  installation, and launch settings.
- **A source project is not an installed release.** Source belongs in an
  editable workspace. Packaging produces a runtime artifact; install and repair
  operate on the artifact.
- **The public registry is not Steam Workshop.** They are separate provider and
  authority boundaries. Do not infer that a content item is published to every
  source because it exists in one.
- **A local deploy is not a Steam release.** Local deployment is for iteration.
  Real Workshop and app-root publishing are separate, reviewed, explicitly
  confirmed operations.

These distinctions are safeguards for authors and players, not extra chores to
expose in every screen. The UI should translate them into simple choices such
as Play, Add to composition, Install, Update, Repair, or Publish, and show the
technical details only when they help resolve a real decision.

