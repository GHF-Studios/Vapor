# SDK, Launcher, Studio, IDE, And Shell

Vapor provides one service and semantic core through several purpose-focused
frontends. A frontend selects and presents workflows; it does not own a
separate content catalog or lifecycle implementation.

The approved product direction is:

- **Launcher:** player discovery, subscription, install into a Composition,
  update, repair, and Play.
- **Studio:** creator templates, content composition, validation, local run,
  artifact review, and Workshop publication.
- **IDE:** source navigation and code editing, with managed developer
  operations for an explicit workspace.
- **CLI/Shell:** support, automation, and recovery access to the same typed
  service operations.

The service owns content and workspace state, provider access, builds,
resolution, installation, progress, and results. This keeps identities and
operation behavior consistent when a player installs content, a creator edits
it, or an author publishes a new release.

The current GUI is still Vapor Studio and includes player areas alongside
creator tools. The separate Launcher and IDE are approved targets but are not
yet shipped as independent applications. Steam entrypoint mapping is also
subject to an implementation review of the deployed manifests.
