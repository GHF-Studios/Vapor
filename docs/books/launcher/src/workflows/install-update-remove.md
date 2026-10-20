# Install, Update, Remove

These actions apply to a Composition's resolved graph. They are not synonyms
for subscribing to a Workshop item or deleting an arbitrary directory.

## Install

Install resolves the selected root Packagepack and its dependency requirements
to concrete content releases. Vapor acquires missing payloads from configured
providers, verifies the staged files, writes the installation state, and makes
the Composition ready to launch. The UI should report what it is installing
and any required choices before making changes.

If an item is already in the Library, Vapor can use that local copy after
checking its identity and payload. If the item is only subscribed to in Steam,
the Steam client may still need to complete its download before Vapor can
import it.

## Update

Update checks for newer compatible releases and compares the candidate graph
with the installed one. The user should see which versions or dependencies
would change, then apply the update. The resolved installation record keeps the
exact versions used by the previous install until the update succeeds, so a
failed acquisition does not turn the launch into a half-updated mixture.

Auto-Install is a content-level policy that governs automatic version updates
and cache collection. It is distinct from resolving required dependencies when
the player explicitly installs a Composition.

## Remove

Removing a Composition stops it being a launchable setup and clears its
composition-owned installation state according to the requested action. A
shared release must remain available while another Composition still depends
on it. Content intentionally kept in the Library must also remain available
after its last active use. The service, not the UI, decides whether an artifact
is safe to collect.

Unsubscribing from Workshop changes Steam's subscription. It is a separate
choice from removing local content, because another Composition may still need
the local payload and the user may want to keep it for later.

## If something fails

Vapor reports which stage failed and preserves known-good installed state where
possible. Refresh source and on-device discovery if the local catalog is stale.
Use [Repair](repair.md) for damaged installed files. Do not manually delete
files from the Steam app root or local Library to work around a conflict; that
can discard the evidence needed to find a valid copy.

