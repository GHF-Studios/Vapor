# Trust Model

Vapor's checks answer whether declared content identities and dependency
constraints are internally consistent and whether staged files match recorded
fingerprints. They do not sandbox native code or certify that an author is
trustworthy.

Steam and registry data provide different kinds of evidence: Steam associates
a Workshop item with an account and reports provider state; the registry
validates namespace and source claims. Neither prevents a published native
program from containing bugs or harmful behavior.

Before installing unfamiliar content, review its author, source, version,
dependencies, and required role. A Packagepack may cause the game process to
load native code. Keep the player interface clear about provenance and
compatibility, and keep creator publication explicit and reviewable.
