# Fingerprints

Vapor fingerprints the staged artifact after building and packaging declared
runtime outputs. The fingerprint identifies the exact payload that was reviewed
for publication. If the source or generated files change after review, the
operation must stage and validate the new bytes again.

A fingerprint helps compare, verify, and recover artifacts. It is not a
signature, malware scan, source review, or proof that a program behaves safely.
Native binaries still execute with the permissions of the game process.

The publisher should review the staged files and dependencies, not infer
payload contents from source names or a previous upload. Installed verification
compares local files with Vapor's recorded artifact state and reports
tampering or damage so that repair can restore a trusted copy.
