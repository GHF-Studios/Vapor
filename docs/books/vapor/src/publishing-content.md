# Publishing Content

Workshop publication creates or updates a Steam item containing a staged Vapor
release. The publish operation should start from an authored Workspace, build
and validate its declared runtime outputs, package the selected content, show
the actual payload and dependencies for review, then require an explicit
confirmation before upload.

## Preview first

Use a dry run or Studio's review flow to check identity, version, visibility,
dependencies, platforms, files, and the payload fingerprint. A dry run may build
and stage packages and provider metadata, but it does not upload to Steam. Fix
the source and preview again if any of these facts are wrong.

The Workshop release is one logical content item containing the declared
runtime matrix. Vapor does not split the Linux and Windows payload into
different Workshop items. The staged manifest records which files belong to
which target so the runtime can select the appropriate payload.

## Publish the dependency graph in order

On first publication, dependency items need their Workshop identities before a
dependent item can refer to them. Create and publish dependencies first, repair
the authored dependency IDs, then publish the packs that select them. On later
updates, stage the complete graph and publish the affected items together where
appropriate.

The Shell example below is a preview only:

```text
content publish ghf-studios/loo-cast/spacetime-engine ghf-studios/loo-cast/loo-cast-game ghf-studios/loo-cast/loo-cast-packagepack --dry-run
```

Real create, update, or delete operations must remain interactive and
confirmation-bound. Scripts cannot bypass that boundary. The Steam client must
be available, the selected account must have permission, and the workspace
runtime matrix must include the required Linux and Windows targets.

## Review the bytes that will be uploaded

Studio's review is tied to the staged payload fingerprint. Before upload, the
service stages the source again and compares the result with the review. If the
source changed, publication stops. When it matches, the service uploads an
operation-owned copy of those exact staged bytes. This protects against a
time-of-check/time-of-use change between preview and upload.

After a successful publication, record the returned Workshop identity in the
authored metadata and refresh source facts. The published release can then
appear in discovery and be acquired into a composition's Library through the
player lifecycle. Upload success is not proof that the item installs or runs on
every platform; validate both target builds and exercise the installed item
separately.

