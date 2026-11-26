# Publish The Loo Cast Steam Packagepack

The generic `root publish` command stages and publishes the Loo Cast
Packagepack Steam depots in the Loo Cast source checkout. It does not publish
Vapor itself, which has an independent release path, or Workshop content.

Start with a preview:

```text
root publish --dry-run
```

Review the validation, full Linux/Windows target matrix, shared manifest,
per-target executable and declared runtime sidecars, and branch. Confirm no
Vapor-owned files are staged. A real upload requires the dedicated Steam build
account and explicit confirmation:

```text
root publish --account ACCOUNT --branch vapor-dev --yes
```

Use the actual configured beta branch for the project. Real publication
rejects host-only or custom target subsets because a release must include the
full declared runtime matrix. Engine, Game, Mod, and Packagepack community
artifacts use `content publish` only when their configured content source is
Workshop.
