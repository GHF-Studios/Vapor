# Ecosystem Map

Vapor is a family of repositories with different jobs. The same content
identity and lifecycle should travel across them; repository layout alone does
not define what an item means.

```text
Player ───────── Vapor Launcher ──┐
Creator ──────── Vapor Studio ────┤
Code author ──── Vapor IDE ───────┼── local Vapor Client service
Support/dev ─── Vapor CLI/Shell ──┘             │
                                             ├── Vapor core and providers
                                             ├── local compositions and Library
                                             └── Steamworks / Workshop

Creator workspace ── build and stage ──> versioned content release
                                              │
                               Workshop / future registries

Vapor Platform Server ── homepage, docs, identity, registry, diagnostics
```

## Who owns what

- **Vapor** holds shared Rust code: typed content identity, manifests,
  resolution rules, managed toolchain and workspace operations, local state,
  service protocol, and provider adapters.
- **Vapor Client** assembles the application and its front ends. Its local
  service is the stateful authority for compositions, Library state, workspace
  operations, and lifecycle requests. GUI and CLI facades should present the
  same operation rather than implementing parallel business rules.
- **Vapor Launcher, Studio, and IDE** are the owner-approved product
  boundaries: player, creator, and code-authoring experiences. The current
  GUI still bundles player and creator workflows; the split is not yet shipped.
  Their Steam entrypoint mapping must be checked against the deployed root
  manifest rather than inferred from product names.
- **Vapor CLI / Shell** exposes direct commands over the same managed operation
  contracts. It is useful for development, recovery, and automation. Backend
  providers such as Git, Cargo, SteamCMD, and Steamworks are controlled through
  Vapor rather than treated as unrelated public command tools.
- **Vapor-Examples** contains example and demonstration content. First-party
  Loo-Cast content is authored as a normal workspace and uses the same content
  and Workshop lifecycle as other game content.
- **Vapor Platform Server** composes independent public services such as
  Homepage, Docs, Identity, Registry, and Diagnostics behind one domain. These
  services own their data and can be rebuilt independently; the root repository
  owns orchestration rather than absorbing their business logic.
- **Steam** provides app delivery, launch entrypoints, user identity context,
  and Workshop discovery/subscription/download/upload. It does not replace the
  local Vapor Library, composition lock, or installation verification.
- **GitHub** hosts source repositories and supports developer identity and
  source workflows. It is not the runtime content cache and is not a substitute
  for Workshop installation.

## Follow one item through the system

An author edits a registered source project. The Vapor service invokes its
pinned build toolchain, stages the declared runtime outputs, and records a
fingerprint of the result. The selected publisher sends that reviewed payload
through Workshop. A player's Steam client downloads it; Vapor then imports it
into the local Library, resolves it with compatible dependencies, and installs
the exact graph selected by a Composition. Studio and Shell can both inspect or
request lifecycle actions because they use the same Client service.

Each boundary has a distinct responsibility: Steam moves Workshop bytes,
Vapor interprets content identities and dependencies, the service owns local
operations and state, and each front end explains the available user choices.
This is how the ecosystem stays coherent as more tools and content types are
added.

