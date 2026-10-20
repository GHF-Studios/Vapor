# Publish To Workshop

Workshop publication sends one logical content artifact with its declared
runtime matrix and dependencies. First make sure the source workspace validates
and the runtime targets include the intended Linux and Windows platforms.

For first publication, create dependencies before dependants and repair the
newly assigned Workshop IDs between each creation:

```text
content validate
content create ORG/NAME/NAME-engine --account ACCOUNT --yes
source repair --write
content create ORG/NAME/NAME-game --account ACCOUNT --yes
source repair --write
content create ORG/NAME/NAME-packagepack --account ACCOUNT --yes
```

Before a real create or update, inspect a dry-run in the interactive shell:

```text
content create ORG/NAME/NAME-packagepack --dry-run
content publish ORG/NAME/NAME-packagepack --dry-run
```

For updates, the artifact must already have a PublishedFileId in its role
manifest. Publish related artifacts in one explicit batch only after reviewing
the staged identities and dependencies. Real publication needs Steamworks
access and is intentionally separate from local deployment.
