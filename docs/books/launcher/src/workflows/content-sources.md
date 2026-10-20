# Content Sources

Vapor can discover content from more than one place. The source tells you who
provided a release and how the local copy arrived; it does not change the
content's Vapor identity or kind.

| Source | What it represents | Player-facing meaning |
| --- | --- | --- |
| Steam Workshop | Published content and Steam subscription/download state | Find, follow, download, and update a published item. |
| Local Library | Cached or otherwise locally available releases | Content already available on this device. |
| Installed Composition | The exact graph currently installed for a Composition | Content used by this launchable setup. |
| Workspace | Editable source discovered in a registered project | Creator source; it is not the same as an installed release. |
| First-party source | Content supplied by the game or its official project | Official content with the same identity and dependency rules. |

The same release can be observed in more than one location. Vapor compares the
identity, version, and payload fingerprint rather than assuming that two copies
agree because their IDs match. If local copies conflict, the detail view should
show the conflict and block ambiguous actions until it is resolved.

## Workshop subscriptions versus Library availability

Steam owns the current-user subscription state. Vapor owns the local Library
and composition state. Subscribing does not itself prove that Steam finished
downloading the item, that Vapor imported it, or that a composition selected
it. The UI should report these stages as normal actions and results: following
on Steam, downloading, available in Library, selected for a composition, and
installed.

## Workspaces and local source

A Workspace is an editable authoring context registered by
`Workspace.vapor.toml`. Players do not need a source checkout to install a
published release. Creators can run local source and create local packages;
those operations use the managed developer toolchain and remain separate from
the ordinary player path.

