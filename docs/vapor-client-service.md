# Vapor Client: shared application and service contract

## Product model

Vapor is one content platform with several interfaces. The terminal CLI, Studio,
Installer, Steam launch path, and future language tools are adapters over one
local Vapor Client service. They use the same Rust contracts and workflow
authority; each interface owns presentation and input only. A workflow must
not acquire a second meaning because it was started from a different frontend.

The local service is a normal user process, not an OS daemon. It is shared by
frontends for the active Vapor installation and owns application context,
durable user state, operation lifecycles, and provider access. A shared
bootstrap/connection manager is the only code that discovers, starts,
connects, recovers, or shuts down that process. The Steam entrypoint invokes
that manager before handing off to the selected frontend; direct CLI or GUI
launches use the same manager. Entrypoints do not implement service startup or
workflow policy themselves.

## Semantic ownership

- **`vapor_content`** defines content and pack vocabulary, stable identities,
  dependency rules, and role-manifest semantics without host or CLI
  dependencies. `vapor_core` currently re-exports those types for its existing
  host operations; that compatibility surface is not a second authority.
- **`vapor_client_domain`** is the host-independent owner for application
  policy and workflows over that vocabulary. It now owns composition records
  and transitions, durable client state, Library Auto-Install policy and
  constraints, catalog reconciliation, and the install-plan result contract.
  Its provider-driven install planner uses a candidate-selection port.
  Filesystem, cache, installed-content, source-workspace, and Workshop
  operations now have one implementation in `vapor_client_provider`, consumed
  by Studio and CLI. The Vapor-owned `vapor_client_service` host owns durable
  persistence and request orchestration; `vapor_client_application` adapts
  domain rules to provider operations.
- **The service API** is the canonical typed boundary for commands, snapshots,
  events, failures, revisions, and operation identity. The local Rust API and
  IPC transport expose the same contract. UI-specific focus, layout, and
  unsaved presentation state stay in each frontend.
- **Studio, CLI, Installer, Steam entrypoint, and future tools** submit the
  same semantic actions and render the same authoritative results. A frontend
  may choose its interaction, but it cannot duplicate policy or claim success
  before the service reports it.

## Domain invariants

- A playable composition is a Packagepack-backed, launchable content graph.
  Composition records are the durable product model. Former profile-shaped
  state is accepted only at the explicit schema migration boundary.
- Discovering, subscribing to, acquiring, installing, adding to a composition,
  and launching content are distinct operations with explicit results.
  Subscription adds availability to the library; it never silently edits a
  composition. Adding an add-on requires a compatible composition target.
- The Rust-defined content and pack vocabulary is the only type vocabulary.
  Creator flows present those types and their implications instead of
  inventing a parallel model. `Crate` is a Rust-only static build dependency,
  not runtime-loaded game content and never a Packagepack member.
- The acquired-content Library is a shared domain projection of locally
  available non-Packagepack content. Cached/downloaded content appears there
  before installation; complete playable Packagepacks appear as compositions.
- One canonical Superworkspace defaults to `~/Vapor`. Its location is explicit
  and changeable. Source acquisition, project registration, and authoring use
  that shared context.
- Availability is derived from content compatibility, installation state,
  host capabilities, and provider state. Operations have stable identity,
  observable progress, terminal results, and recoverable errors.

## Application domain and state ownership

These aggregates have different identities and lifetimes; a single “profile”
or “installed content” record must not stand in for all of them.

| Aggregate | Owns | Lifetime and authority |
| --- | --- | --- |
| **Content** | Canonical identity, artifact kind, version, compatibility, and dependency declarations | Defined by source content and Vapor's shared resolver; the service reports resolved meaning |
| **Composition** | User-owned playable setup: stable identity/name, Packagepack selector, launch settings, and optional source-workspace association | Durable user intent; it exists independently of whether its content is installed |
| **Composition resolution** | A complete Packagepack graph with exact content identities and effective Engine/Game | Computed by Vapor's resolver from composition intent. Whether exact dependency versions are pinned or refreshed is an explicit update policy |
| **Install plan** | For each resolved node, eligible source, availability, blockers, and required acquisition step | Computed per operation from resolver output plus workspace/cache/installed/provider availability; not composition identity |
| **Composition installation** | Installed artifacts, runtime paths, fingerprints, and verification state for one resolved composition | Service-managed materialization; reconciled against resolved content and host state |
| **Library entry** | Non-Packagepack content acquired or cached locally, its source, and next valid action | Shared service projection of availability; may be cached but not installed, separate from Workshop subscription and composition membership |
| **Workshop relationship** | Provider subscription and provider-reported item identity/state | Owned by Steam/Workshop; locally observed and reconciled by the service |
| **Workspace/source** | Canonical Superworkspace and registered authored repositories | Durable developer context; repository acquisition is an explicit Vapor operation |
| **Operation** | Requested intent, target identity, progress, cancellation support, and terminal result | Service-owned while active; result history is durable only where it supports recovery/audit |
| **Frontend view state** | Selection, focus, search text not submitted, layout, and unsaved presentation drafts | Owned by the frontend; never authoritative for installation or provider state |

Composition definition, resolved content graph, per-operation install plan,
installed outputs, current selection, library availability, Workshop
subscription, and source repository status are separate facts. The service
may project them together for a screen, but must preserve their ownership and
update rules. Technical cache records and fingerprints support these
aggregates; they are not player-facing domain concepts.

This pass closed the first protocol boundary: composition operations now use
one `CompositionAction` family and `composition_id`; snapshots and durable
writes expose `CompositionDocument` and composition selection/revision fields.
Protocol version 9 rejects older wire peers. Durable schema 2 accepts the
former profile keys only while decoding schema 1 and atomically rewrites them
to composition fields. A regression fixture proves composition identity,
Packagepack, and launch arguments survive this upgrade. Composition identity
references now use the shared `vapor_content::ContentReference`, whose
deserialization validates IDs and exact versions. The protocol's graph record
has checked conversions to and from the shared resolved composition; its
source and fingerprint fields remain installation observations, not resolver
inputs.

The current crate graph also exposed directly evidenced semantic splits. One
was resolved in this pass: `vapor_core::ContentKind` is now the CLI manifest
and content-operation type as well, and canonical manifest names and
dependency-kind rules live with it. The CLI recognizes the canonical Rust-only `Crate`
kind rather than maintaining a separate subset. `Library` names the separate
acquired-content collection and is not a content kind. The protocol now
serializes a checked projection of the shared graph; it does not choose
versions or validate dependency compatibility. Its per-node source and
fingerprint fields are observations used to reconcile installed materialized
artifacts.

### Resolution and crate-ownership audit

Library ownership is distinct from dependency resolution. A Library item can
be user-managed with Auto-Install or explicitly retained. Auto-Install means
Vapor automatically selects and installs the newest version that satisfies
every dependent composition, then removes the local download when no
composition needs it. Retained items stay locally available until the user
explicitly updates or removes them.
Retained items remain available until an explicit update or removal. This is
local user policy owned by the application service; it is not an authored
manifest field, a Workshop subscription, or an install-time dependency flag.
The resolver selects a composition graph; the lifecycle manager applies that
graph to Library policy and provider availability facts. The current code has
the durable preference and version/reachability policy, but automatic update
and collection execution remains open work.

The existing `vapor_core` crate is not yet a narrow semantic core: its public
surface also includes CLI dispatch, Cargo realization, managed tools,
installation, Steam deployment, profiling, and development-environment
orchestration. The current ownership, based on direct imports and call sites,
is:

| Current code | Observed responsibility | Intended owner |
| --- | --- | --- |
| `vapor_content::{identity,content,authored_manifest,catalog,resolution}` | IDs, content kinds, pack/dependency rules, typed authored role schema, exact candidates, and generic graph resolution | Canonical host-independent content domain |
| `vapor_client_domain::{composition,library}` | Composition records and intents, naming/duplication/removal rules, graph identity checks, install observations, and per-item Auto-Install/update/collection policy and intent | Shared durable lifecycle; protocol wraps/re-exports the same records and intents |
| `vapor_core::{manifest,local,resolution}` | Provisional generic manifest, filesystem discovery, and adapter into shared graph resolution | Provider adapter and compatibility facade; not a second resolver |
| `vapor_core::{cli,cargo,development,installation,steam,toolchain,...}` | Commands, host effects, build/install/deploy, and developer operations | Vapor service/provider infrastructure; not the semantic content core |
| `vapor_client_provider::content::preview_install` | Supplies workspace/cache/installed/Workshop candidate availability and provenance | `vapor_client_domain` owns traversal, exact-edge checks, readiness, and plan output |
| `vapor_client_application::{catalog,content,workspace,publishing}` | Application operations and source-workspace adapters over shared content/provider operations | Vapor Client application API; presentation stays in Studio |
| `vapor_client_protocol` | Wire-safe action/result/snapshot types and validation at the transport boundary | Serialization projection of shared domain records and install observations |

The old catalogs consume different valid inputs: `vapor_core::LocalCatalog`
reads the generic `Vapor.toml` fixture format while Vapor CLI reads
role-specific manifests such as `Engine.vapor.toml` and
`Packagepack.vapor.toml`. Both now
adapt resolved candidates to `vapor_content::ContentCandidates`, and the core
resolver uses the shared exact-graph and pack-validation algorithms. The
Vapor-owned `vapor_client_provider` supplies candidate availability; the
shared planner owns traversal, readiness, and plan output, including workspace,
cache, installed, and Workshop-seed provenance. Workspace and Workshop effects
also use that provider implementation. The service host, durable state, and
request orchestration have moved into Vapor. Studio and Steam EntryPoint now
share the per-installation local IPC service; the stdio endpoint remains only
for isolated service simulation.

The ownership extraction is complete at the crate boundary:

1. Content intake uses the typed shared role-manifest parser; the generic
   `vapor_core::LocalCatalog` remains a separate adapter into the shared graph.
2. `vapor_content` owns graph meaning and resolution; `vapor_client_domain`
   owns the install plan and lifecycle policy; `vapor_client_provider` supplies
   availability and provenance.
3. Filesystem and Workshop effects have one provider implementation in Vapor.
   CLI and Studio consume it as frontends; neither owns a competing resolver.
4. The Vapor-owned application and service-host crates now own workflows,
   persistence, request orchestration, and process coordination.

### Canonical ownership map

| Concern | Authority | Projection or adapter |
| --- | --- | --- |
| Content IDs, versions, kinds, pack membership, authored role schema | `vapor_content` | CLI parsing and protocol encoding |
| Dependency compatibility and exact graph resolution | shared Vapor content semantics | provider-neutral catalog input; graph preview |
| Workspace/cache/installed/Workshop availability and provenance | `vapor_client_provider` | install plan and user-facing availability |
| Compositions, Library ownership policy, workspace context, operations, persistence | Vapor Client application service | `vapor_client_protocol` snapshots and actions |
| Wire framing and transport-safe request/result/event structures | `vapor_client_protocol` | client facade and service adapter |
| CLI command grammar and terminal presentation | Vapor CLI | target: submit shared client actions; current commands still call providers directly |
| Studio screens, labels, layout, and interaction state | Vapor Studio | calls the shared service API |
| Filesystem, Steam Workshop, Cargo, and process effects | narrowly owned provider adapters | invoked by application operations |

The current request vocabulary maps to that model as follows:

| Current API family | Domain concept | Frontend-only portion |
| --- | --- | --- |
| `CatalogAction` | Library discovery, inspection, acquisition availability, and candidate adoption | Search text and selected row |
| `CompositionAction` | Composition intent, resolution, materialization, verification, repair, update, and launch | Active composition selection and launch confirmation UI |
| `WorkspaceAction` | Active authoring context, source-project registry, and bounded file operations | Tree expansion, editor focus, and unsaved presentation state |
| `AuthoringAction` | Source composition, artifact metadata, validation, and creation | Form state and editor controls |
| `WorkshopAction` | Search/subscription relationship, acquisition, and reviewed item publication | Query text, page selection, and review-dialog state |
| `PublishingAction` | Reviewed first-party application/depot publication | Confirmation presentation |
| `DeveloperAction` | Explicit developer operations delegated to managed build/tool providers | Developer-only navigation and confirmation UI |
| `SessionAction` | Recovery checkpoint of editor drafts and operation history | Draft serialization format only |

This mapping assigns meanings and gives shared content/provider operations one
Vapor-owned implementation. The service host, durable state, and request
orchestration are also Vapor-owned. Pass 1's ownership review is complete:
content semantics, application policy/planning, local provider operations, and
service-host responsibilities each have one Vapor-owned implementation, and
Studio no longer depends on CLI.

The shared-service process coordination now uses one local service for Studio
and Steam EntryPoint. The `vapor client launch` command reaches the service
manager through EntryPoint. CLI content operations still call providers
directly; routing those workflows through the shared service is later work.

Workspace context also has two distinct meanings: the active authoring folder
is a changeable editing target, while the canonical Superworkspace is the
source-acquisition and repository-discovery home (default `~/Vapor`). Neither
is a composition's optional source binding. The service now initializes the
default Superworkspace on first real frontend connection, exposes its state
and repository discovery, and accepts explicit location changes. These
operations do not change the active authoring folder. Durable authoring context
is named `authoring_workspace_root`, while the old `workspace_root` key remains
a read alias. Repository acquisition through the service and path-free source
selection remain to be completed.

Studio now has a separate player-facing Library view over the shared catalog;
it shows downloaded or installed runtime content, excludes Packagepacks and
Rust-only Crates, and opens after successful Workshop acquisition. Catalog
identity reconciliation and install availability are shared domain/provider
contracts. The service owns process state and operation persistence, and the
client now supports multiple connections with request correlation and snapshot
recovery. Command parsing and terminal presentation stay in Vapor CLI.

## Process and API contract

The service has one host context per process and many independent frontend
connections. Entrypoint readiness checks do not initialize durable state. The
first frontend connection may carry the old GUI's saved-state seed; the service
decides whether the state file needs importing and performs that migration
itself. Once durable state exists, later connections' seeds are ignored. The
frontend never writes the service's durable state directly.

The shared bootstrap manager guarantees one active service for the selected
Vapor installation, handles stale endpoints and incompatible protocol
versions, and returns a connection to each frontend. The service owns
authorization, request validation, operation coordination, persistence, and
provider-side effects. Requests carry caller identity and expected revision
where concurrency matters. Responses correlate to requests; events carry
operation progress and state changes. Reconnection obtains a fresh snapshot
without repeating mutations.

The Steam entrypoint owns executable-root discovery, launch-mode parsing,
launch logging, calling the shared service bootstrap, and frontend handoff. It
does not contain a second service manager or application workflow. The common
manager, not an individual entrypoint, owns singleton and process-lifetime
rules.

## Repository and crate boundaries

- `Vapor/crates/vapor_core`: shared content semantics and provider-neutral
  resolution. It currently still owns broad host, CLI, and Cargo operations;
  those are not part of the final narrow content boundary.
- `Vapor/crates/vapor_client_domain`: host-independent application rules,
  durable composition and Library policy, catalog reconciliation, and shared
  install planning. The service host still needs to own durable execution and
  lifecycle operations.
- `Vapor/crates/vapor_client_protocol`: canonical transport-safe request,
  response, snapshot, event, operation, and failure types. Composition and
  installation records are re-exported from `vapor_client_domain`, not
  independently defined wire types.
- `Vapor/crates/vapor_client`: shared Rust API and frontend connection/bootstrap
  facade. Process startup and IPC lifecycle live here or in a directly-owned
  service-host crate under Vapor, never in GUI-specific code.
- `Vapor`: owns the Vapor Client application domain, local provider
  implementations, client facade, and service-host implementation. Shared
  provider/application/service crates must not depend on CLI or GUI crates.
- `Vapor-Studio`, `Vapor-CLI`, `Vapor-GUI`, `Vapor-Installer`, and
  `Vapor-Steam-Entrypoint`: interface adapters and packaging surfaces over the
  same service API.

## Implementation order and current gap

This is the intended application model from the outset. Porting existing code
is an implementation task, not a reason to call the API temporary or make the
old Studio boundary canonical.

1. Define the composition/content/workspace domain and its invariants in shared
   Rust contracts. Remove Studio-specific type names and compatibility aliases
   from the service API.
2. Move workflow authority, provider ports, application operations, and the
   service host into Vapor. The service and application crates now live there;
   Studio no longer owns either implementation.
3. Implement one service instance with multi-client local IPC, a process-level
   bootstrap contract, durable state ownership, and explicit connection
   lifecycle. Vapor Client now owns the local-socket manager, protocol
   negotiation, stale-endpoint recovery, reconnect, and idle shutdown. Stdio is
   reserved for isolated service simulation.
4. Route Studio, CLI, Installer, and Steam launch through the same client API
   and bootstrap manager; remove each duplicated operation path as its service
   route becomes authoritative.
5. Verify singleton behavior, concurrent frontends, reconnect and recovery,
   version mismatch handling, persistence across facade restarts, and packaged
   Linux/Windows launch paths through Vapor-managed tooling.

The current checkout has moved the client/protocol, application, local provider,
and service-host crates into Vapor. Studio UI consumes Vapor-owned application
contracts, and Vapor CLI re-exports shared provider operations as a frontend
facade. Studio and Steam EntryPoint now attach to the same local service for a
given installation and state directory. Linux local deployment compiled and
deployed the service changes; runtime concurrency and Windows behavior remain
unverified.
