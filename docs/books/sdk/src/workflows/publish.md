# Publish

Vapor has two distinct publishing authorities:

- Root publishing stages and uploads the Vapor application/depot through
  SteamPipe.
- Content creation and publishing create or update individual Steam Workshop
  artifacts.

For either path, begin with a dry-run and inspect identity, branch or
PublishedFileId, targets, staged files, and dependencies. A preview does not
prove that the Steam account has permission or that a live client will launch
the result.

Root publishing requires the configured full runtime matrix and a non-default
beta branch. Workshop publishing requires valid authored metadata and the
complete declared Linux and Windows runtime matrix. Host-only and custom
target subsets are local-preview options; real upload rejects them.

A real upload is an explicit, authority-changing operation. It requires
account selection and confirmation in the interactive shell. Never put Steam
passwords or guard codes in manifests, scripts, or source control. The
Distribution Book explains authority and release semantics.
