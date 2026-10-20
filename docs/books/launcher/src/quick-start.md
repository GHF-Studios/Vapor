# Quick Start

This is the intended player journey. Steam-backed Workshop search, subscription,
and download need a running Steam client signed into the intended account.
Already installed content and local Compositions remain usable when Workshop
is unavailable.

## Play a Composition

Choose a Composition in Library and select **Play**. If it needs content, the
Launcher should explain the exact missing items and offer **Install** or
**Update** before starting. A Packagepack is the root of a playable setup; it
selects the required Engine and Game choices.

Until the separate Launcher ships, these actions are available in the Play and
Library areas of Vapor Studio. The shell equivalent for the configured default
composition is:

```text
launch default
```

The shared service verifies and starts the resolved composition. If it reports
that the composition needs attention, resolve that state before retrying Play.

## Find and Install Content

Search in Discover, open an item's details, and review its kind, source,
version, compatibility, and dependencies. **Subscribe** follows a Workshop item;
**Install** makes it available locally and adds it to an eligible Composition.
The Launcher should show eligible Compositions as destinations and explain
which are editable or managed.

A Packagepack can be the Composition root. An Engine, Game, or Mod belongs in a
compatible slot or pack; it cannot become a playable root merely because it is
downloaded. Subscription, local Library availability, and Composition
membership are distinct.

## Keep the setup stable

An installed Composition records resolved content versions. Discovering a new
Workshop release does not by itself change the running setup. Use **Update**
to review and apply newer versions. Auto-Install content follows its configured
update and cache-retention policy; intentionally retained content stays
available even when no Composition currently needs it.

Continue with Install, Update, Remove and Lock And Launch in the Launcher book.
