# Package

Packaging creates a reviewable artifact root from declared source and runtime
outputs. It is not an upload and does not change the user's installed
Composition.

For content:

```text
content validate
content package ORG/NAME/NAME-packagepack --dry-run
```

The service builds the source workspace, stages the selected artifact, writes
a resolved deployed manifest, and fingerprints staged files. With a declared
runtime matrix, one package contains platform outputs in target-specific
directories. Host-only is for local smoke work, not Workshop publication.

For the Vapor application root:

```text
root package
```

Root packaging assembles and checks the app/depot payload without SteamCMD
upload. Content packages and root-app packages have different identities and
publication paths; do not put the game composition into the Vapor app depot.
