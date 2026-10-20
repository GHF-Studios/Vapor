# Publish The Root App

Root publishing updates Vapor's Steam application/depot, not Workshop game
content. It validates, builds the declared runtime matrix, stages the depots,
and can upload the result to a configured non-default branch.

Start with a preview:

```text
root publish --dry-run
```

Review the validation, target matrix, staged payload, and branch. A real
upload requires the dedicated Steam build account and explicit confirmation:

```text
root publish --account ACCOUNT --branch vapor-dev --yes
```

Use the actual configured beta branch for the project. Real publication
rejects host-only or custom target subsets because a release must include the
full declared runtime matrix. Do not use this command to publish an Engine,
Game, Mod, or Packagepack; use `content publish` for those Workshop artifacts.
