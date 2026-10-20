# Limits

Vapor validates declared identities, version constraints, compatibility,
dependencies, and staged files. Those checks do not prove that content is safe,
well-designed, or free of runtime bugs. Native Rust code runs with the
permissions of the game process; a manifest is not a sandbox.

The current product also has concrete gaps:

- The player Launcher, creator Studio, and code-authoring IDE are approved
  separate frontends, but are not yet independently shipped.
- Source synchronization is not implemented by the Shell source-sync command.
- Workshop access depends on Steam client/provider availability; subscription
  does not guarantee a payload has been acquired.
- A host-only deploy does not prove another operating system works.
- A binary-only Workshop artifact cannot be forked into editable Rust source.
- Generated docs and metadata cannot replace runtime verification or release
  review.

The product must show these boundaries when they affect a user's decision.
A verified fingerprint is not a security audit, and successful publication
does not mean content works on every target.
