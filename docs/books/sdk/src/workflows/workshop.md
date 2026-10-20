# Workshop

Steam Workshop is both a discovery/provider source and a publication target.
The same artifact identity follows content from authored workspace to staged
release, Workshop item, Library, and Composition.

For a new dependency graph, publish dependencies before the Packagepack.
Steam assigns a PublishedFileId to each created item; use source repair to fill
those IDs into authored dependencies before creating the dependent item:

```text
content validate
content create ORG/NAME/NAME-engine --dry-run
content create ORG/NAME/NAME-engine --account ACCOUNT --yes
source repair --write
```

Repeat for the Game and Packagepack. For an existing item, use content publish
after reviewing its staged artifact. One Workshop item contains its declared
runtime targets; do not split Linux and Windows into separate items.

Player subscription and acquisition use the Steam client provider. Authoring
and publication additionally require Steamworks permissions and managed
release tools. Subscribed does not mean downloaded, installed, or added to a
Composition.
