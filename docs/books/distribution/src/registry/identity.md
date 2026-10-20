# Identity

A content artifact has a stable fully qualified identity and a version. The
identity names its organization, workspace, and project; the version identifies
one release. A Steam PublishedFileId is a provider identifier that connects
that content identity to one Workshop item. It is not a replacement for the
Vapor identity.

A player can encounter the same artifact through Workshop search, a local
source workspace, an installed cache, or a Packagepack dependency. Vapor uses
the stable identity to connect those records and provenance to show where the
artifact came from.

Do not rename an artifact to represent a new version. Increment the version
and keep the same identity when updating the same project. Use a new identity
for an intentional fork or a different project. The content manifest and
registry rules define how the identity is validated.
