# Local Development Content

Creators can run local source or deploy a local package into the installed
application for a private smoke pass. This is not a Workshop upload and does
not replace the source checkout with a built artifact.

Local source is discovered from an explicitly opened Workspace. The installed
app owns the managed Rust toolchain, generated outputs, and runtime state; the
repository owns source. Use `source open PATH` to select a Workspace and
`content validate` to check registered content metadata before building.

When a creator deploys local content, keep its source identity and version
visible and distinguish it from any installed Workshop copy. If multiple local copies of one exact content version disagree, the catalog
reports the conflict and blocks ambiguous use or launch. Publishing is different:
an explicitly reviewed Workspace source is authoritative for the Workshop update,
even when alpha content reuses its version number. Publishing does not change
installed copies. Do not overwrite a cached Workshop payload by hand.

After testing, the creator can stage and review a release from source. The
reviewed Workshop artifact is built from current saved files. Local deploy
success does not imply the release payload is the same; staging verifies the
exact bytes that will be published.

