# Troubleshooting

## Steam Workshop search or download fails

Start Steam and confirm the intended account is signed in. Workshop search,
subscription state, and downloads come from the Steam client. Retry after Steam
reports that the item finished downloading. If it still fails, refresh the
subscription and local content lists, then inspect the item details for an
availability or compatibility error.

## The item is subscribed but not in the Library

Subscription is Steam state; the local Library is Vapor state. Check whether
Steam has finished downloading, then use Download/Acquire in Vapor to import
the payload. A subscription alone does not make a package available to a
Composition.

## The item cannot be added to my Composition

Check its content kind. A Packagepack can be a Composition root; an Engine,
Game, or Mod must fit a supported slot or compatible pack. Refresh the local
catalog if the action is disabled. If multiple copies of the same ID and
version disagree, review and resolve the local conflict first.

## Play says the Composition needs attention

Open the Composition's details and review the resolved graph. Use Update when
new versions should be considered. Use Repair when installed files fail
verification or are missing. If the needed provider is unavailable, connect it
or reacquire the exact release; do not delete random app-root directories.

## The game does not start

First confirm that the Composition has a playable Packagepack and a verified
installation. Try the selected Steam Play entrypoint or `launch default` for
the configured default. If the app was just locally deployed, ensure the Steam
client is running when the game requires its API and that the launch entrypoint
is invoking the expected application mode.

## A new update changed the game unexpectedly

Review whether the content has Auto-Install enabled. That policy permits
automatic version updates. Otherwise, the Composition should retain its
resolved versions until an explicit update. Inspect the operation result to
identify which release changed and which Composition was updated.

