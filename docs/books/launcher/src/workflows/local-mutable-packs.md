# Local Mutable Packs

A local mutable pack is source the user can edit. It is different from a
published Workshop pack and from a downloaded runtime release. To change a
published pack, create a fork or a new source project rather than modifying
provider cache bytes in place.

A fork gets its own content identity and source manifest while retaining its
dependency references unless the author deliberately changes them. Publishing
the fork does not update or unpublish the original item. Before composing a
local fork, ensure its required descendants are also locally authored; Vapor's
workspace boundary rejects a local graph that silently depends on hidden
remote source modifications.

The Launcher should make the choice explicit: **Use this release** for the
published version, or **Create a local copy** to edit and test a fork. It should
show which Composition uses the local source and offer a clear way to return to
the published release. Never silently mutate an installed copy when the user
expects to make an authoring change.

