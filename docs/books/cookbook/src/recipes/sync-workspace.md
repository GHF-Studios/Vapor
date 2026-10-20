# Sync A Workspace

The shell's `source sync` command is currently a status hook, not a Git pull,
clone, or content update operation. It deliberately does not modify source.

```text
source open /path/to/my-content
source status
source sync
```

The command reports the active source identity and root and states that
synchronization has not been applied. Use your normal Git provider for source
control today, then reopen or refresh the source in Vapor and run
`content validate`.

For Workshop content, use the player content lifecycle rather than
`source sync`: subscribe or acquire the Workshop item, then install it into an
eligible composition. Workshop download is not a source-code checkout. The
[Content Sources](../../launcher/workflows/content-sources.html) guide
explains the difference.
