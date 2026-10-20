# Manifest, Lock, Artifact

A source manifest records authored intent. A generated lock records one
resolved selection. A staged artifact records the actual packaged files.
These objects have different authority.

The workspace manifest registers projects and runtime targets. A role manifest
declares content identity, version, dependencies, compatibility, and
publication metadata. Cargo.toml independently owns Rust packages and build
configuration.

Resolution turns the Packagepack's declared requirements into an exact graph
with provenance and versions. Installation realizes that graph locally and
records verification state. Packaging copies declared outputs into a
self-describing artifact root and fingerprints the staged result. Updating or
repairing a Composition should operate on those records through Vapor, not by
editing generated lock files or copying files by hand.

If the staged files differ from the reviewed fingerprint, the publisher must
review a newly staged artifact before uploading.
