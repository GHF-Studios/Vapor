# Publish The Root App

Root publishing updates Vapor's Steam application/depot, not Workshop game
content. It validates, builds the declared runtime matrix, stages the depots,
and can upload the result to a configured non-default branch.

Start with a preview:

```text
root publish --dry-run
```

Review the validation, target matrix, staged payload, and branch. A real
upload requires the dedicated Steam build account, explicit confirmation, and
the designated GitHub Actions distribution job. Local machines can prepare a
plan but cannot create release-profile outputs or upload:

```text
root publish --account ACCOUNT --branch vapor-dev --yes
```

The distribution workflow sets `VAPOR_DISTRIBUTION_JOB=steam-release-v1`. Keep
Steam credentials in that workflow only; the marker is an operational guard,
not an authentication boundary. Use the actual configured beta branch for the
project. Real publication
rejects host-only or custom target subsets because a release must include the
full declared runtime matrix. Do not use this command to publish an Engine,
Game, Mod, or Packagepack; use `content publish` for those Workshop artifacts.
