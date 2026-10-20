# Packs And Composition

A pack describes a reusable relationship between content. A composition is the
player's complete launchable selection. The selected Packagepack is usually the
composition root, but it is only one node in the resulting graph: its required
engine and game choices may themselves resolve through Enginepacks and
Gamepacks, and any node may declare further dependencies.

## Required choices

The current content model requires a Packagepack to provide an engine choice
and a game choice. Each choice may point directly to an Engine or Game, or to
the matching pack kind. An Enginepack resolves to an Engine; a Gamepack
resolves to a Game. A Game and Enginepack require their own engine choice,
while a Gamepack requires its game choice. Modpacks group compatible engine,
game, and extension mods. Vapor checks these relationships and rejects
unresolved required slots, incompatible kinds, missing dependencies, and
dependency cycles.

This gives authors useful reuse without confusing a reusable pack with the
actual selected graph. A Gamepack can be chosen by multiple Packagepacks; each
player composition can independently select a root and resolve the exact
versioned graph that it will install.

## Composition and installation

The product's Composition is the user's durable launchable object. It holds a
name, root Packagepack, launch settings, and the resolved installation state.
The installation records concrete content releases and dependency edges, plus
the payload evidence used to verify the installed files. The composition is
therefore not a second authored Packagepack and not a loose list of Workshop
IDs.

When the user installs or updates a composition, Vapor resolves dependencies,
acquires missing release payloads from configured providers, validates the
graph, and installs the selected versions. A Library item can remain available
without being referenced by a composition. Removing a composition does not
necessarily remove a downloaded item that another composition still needs or
that the user intentionally keeps.

## Version policy and Auto-Install

The resolved graph represents the versions currently selected. A newer
Workshop release may be available, but it does not automatically replace a
composition's selected version unless the content's Auto-Install policy allows
that automatic update behavior. Auto-Install content is eligible for updates
when the provider reports a newer release and is eligible for cache cleanup
once no composition needs it. Content without that policy can remain cached for
intentional use while the author builds another pack or composition.

That policy belongs to content lifecycle metadata and service behavior, not to
an unrelated UI preset. The GUI should explain its effect in everyday terms;
the resolved version graph and cleanup facts remain inspectable when needed.

