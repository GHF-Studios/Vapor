# Installing And Launching Content

Players select a Composition, not an arbitrary directory or a list of Workshop
IDs. A composition has a root Packagepack, launch settings, and a resolved
versioned dependency graph. The Packagepack supplies required engine and game
choices; those choices can be direct content or reusable packs.

## Find an item and decide where it belongs

Browse local and Workshop content from Studio. A detail view should answer what
the item is, where it came from, what versions and platforms it supports, and
what dependencies it needs. Subscribing tells Steam to follow an item;
acquiring it brings the payload into Vapor's local Library. Neither action
should silently rewrite a composition.

A Packagepack can become the root of a new or existing composition. Other
content kinds belong in a compatible dependency slot or pack. Studio should
show which compositions can accept the selected item and whether each is
editable before adding it. This keeps “available on this device” separate from
“selected for this game setup.”

## Install or update a composition

When the user chooses Install or Update, Vapor resolves the required graph,
selects concrete versions, acquires missing payloads from providers, verifies
the staged data, and installs the graph. A composition's resolved state records
the selected release identities and edges. If resolution changes, the user can
review the resulting graph before it becomes the active install.

Auto-Install is a content policy: eligible items can automatically take newer
versions and can be collected from local cache after no composition needs
them. Without that policy, an intentionally retained item stays available for
building or composing even after its current consumers change. This policy
should not be confused with the ordinary act of installing dependencies for a
composition.

## Play and recover

The launcher checks that the selected composition has a playable root and a
current, verified installation. Missing content can be acquired through the
configured provider; a graph that is stale, conflicting, or damaged must be
reviewed or repaired before launch. Repair uses cached or source payloads where
available and reports a missing provider when no valid copy remains.

The current direct Shell entrypoint for the configured default composition is:

```text
launch default
```

The Steam entrypoint can also start the selected composition directly. The GUI
offers composition selection and management. Player-facing navigation should
keep normal actions such as Play, Install, Update, and Repair separate from
developer operations such as editing a workspace or managing app-root tools.

