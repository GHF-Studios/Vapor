# Repair

Repair restores a Composition or cached release when installed files no longer
match their recorded identity and fingerprints. It is a controlled operation
through the Vapor Client service; it should not be a generic “delete and hope”
button.

Vapor verifies the installed artifact against its recorded state. If corruption
is found, the damaged copy can be quarantined. The service then looks for a
known-good source or cached artifact that matches the required release and
reinstalls it. If no valid source remains, repair reports which provider is
missing and leaves the user with a clear next action.

If two local copies of the same content version disagree, do not choose one
based only on its folder name. Inspect the source and fingerprint details,
resolve the ambiguity, then rerun verification. A stale discovery snapshot
should be refreshed before repair so the service does not act on out-of-date
local facts.

Repair does not upgrade the Composition to a newer release unless the selected
update policy explicitly requests that. Integrity recovery and version update
are separate decisions.

