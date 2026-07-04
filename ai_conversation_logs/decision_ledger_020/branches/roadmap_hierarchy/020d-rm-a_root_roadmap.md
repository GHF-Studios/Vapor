Decision Ledger 020d-RM-a: Root Roadmap Hierarchy

Status:
This is a nested roadmap branch under decision ledger 020.
It exists because Q020d-046 was not only a "what next?" answer. It requested a fairly detailed, walled-off topology of roadmaps and sub-roadmaps.

Trunk integration:
`020d_referee_queue.txt` now keeps the answered prefix and the Q020d-046 pivot only. The long flat unanswered tail was preserved at `history/020d_before_roadmap_collapse.txt` and should be treated as source material for nested checkpoint roadmaps or branches, not as an active answer queue.

Carried owner answer:

Q020d-046. Should the immediate next implementation packet be a glossary migration plan, before repo topology/dependency/toolchain implementation work?
A: I- yes, but I wanted more of a roadmap with descriptions and sub-roadmaps and all, yk? Not jus "what to do next". I was hoping to get a fairly detailed and walled-off topology of roadmaps basically, if that makes any sense man. Just markdown files and folder nesting would do the job. So we do need our glossary, and we need a new docs section "roadmap", containing the root roadmap basically, yk?

Referee correction:
The glossary branch is not the whole answer to Q020d-046. It is the first active child workload under a larger roadmap hierarchy.

Canonical target:
The roadmap section is now settled at `Vapor/docs/roadmap/`.
This branch remains the source-trail/history copy for the Q020d-046 pivot and the initial roadmap hierarchy.

Scope control rule:
The main 020 trunk should stop accumulating flat question lists. Broad unresolved areas become roadmap checkpoints. Each checkpoint may open a branch folder only when it needs detailed Q&A, inventory, or implementation packet work.

========================================================================
Root Roadmap
========================================================================

The active design work is now organized as a dependency hierarchy:

1. Documentation substrate
2. Repository topology
3. Steam root and self-bootstrap
4. Toolchain portability and targets
5. Workspace execution and hermeticity
6. Manifest, lockfile, artifact, and fingerprint model
7. Packs, capability, Rhai, and runtime concepts
8. Steam Workshop, registry, trust, and install ledger
9. Command surface and safety
10. Code cleanup and maintainability

The order is dependency-respective, not strictly chronological. A later checkpoint may receive small implementation slices early when it is low-risk and does not decide upstream policy.

========================================================================
1. Documentation Substrate
========================================================================

Purpose:
Create the canonical documentation substrate needed before the rest of the roadmap can stop drifting.

Current active branch:
`decision_ledger_020/branches/glossary_migration/`

Accepted direction:

- The old Loo-Cast glossary content becomes canonical Vapor concept material.
- The old Loo-Cast repo remains legacy/context.
- `Vapor/docs/glossary/` is accepted.
- `Vapor/docs/tech_glossary/` is accepted in principle.
- Migration is selective, not whole-corpus.
- `.obsidian/` is excluded.
- Old AI logs stay history.
- Diagrams, RFCs, and old root docs stay out unless separately approved.
- Imported pages default to `legacy signal`.
- `implemented` requires active code/config representation.
- `WIP` means canonical but not trustworthy/final.
- A `Vapor/docs/README.md` requires an approval packet before creation.

Immediate branch tasks:

- Finish `020d-GM-*` answers enough to approve an inventory.
- Produce a candidate-page inventory: source path, destination bucket, tags, line count, direct links, likely dangling links.
- Draft a migration packet with selected pages, exclusions, status handling, dangling-link handling, and `Vapor/docs/README.md` text.
- After approval, copy the selected pages as one docs-only slice.

Output target:

- `Vapor/docs/README.md`
- `Vapor/docs/glossary/`
- `Vapor/docs/tech_glossary/`
- restrained summary/review files under `Vapor/docs/`

Do not do yet:

- Do not rewrite conceptual content during first copy.
- Do not migrate Spacetime/Loo Cast/USF pages unless required context for active Vapor concepts.
- Do not continue main 020 Active Repo Topology until this branch produces either an approved migration packet or an explicit pause.

========================================================================
2. Repository Topology
========================================================================

Purpose:
Lock the current multi-repo operating model enough that dependencies, docs placement, and root packaging can stop wobbling.

Inputs:

- Documentation substrate branch, because the canonical glossary/roadmap location affects repo docs rules.

Known active repos:

- `Vapor`
- `Vapor-SDK`
- `Vapor-Launcher`
- `Vapor-Examples`
- `Vapor-Registry`

Likely future/related repos:

- `Spacetime-Engine`
- `Loo-Cast-Game`

Unresolved decisions to collapse later:

- Whether Vapor temporarily hosts selected Spacetime/Loo Cast concept pages as `legacy signal`.
- Whether `Vapor/AGENT.md` stack wording needs revision once docs/roadmap exists.
- Whether `Vapor-Registry` is active authority now or active context only.

Potential branch:
`decision_ledger_020/branches/repo_topology/`

Open this branch only if the main trunk cannot answer repo topology in a compact pass after glossary migration.

========================================================================
3. Steam Root And Self-Bootstrap
========================================================================

Purpose:
Make root development operate through the Steam-distributed app root and prove the SDK can maintain the app that ships it.

Inputs:

- Repository topology.
- Toolchain portability.
- Root package policy.

Current code facts:

- AppID: `2122620`.
- DepotID: `2122621`.
- Root package currently includes app-root CLI surfaces and activation scripts.
- Root package currently excludes `rustup-home`, `cargo-home`, and `output`.
- SDK has root package and root publish flows through SteamCMD.

Unresolved decisions to collapse later:

- Whether docs/glossary material is included in the Steam root package.
- Whether the root package eventually includes a full app-local Rust toolchain or only acquisition/reconciliation logic.
- What proof makes the self-bootstrap loop real.
- Whether `sdk_cli` and `launcher_cli` are long-term command names or current aliases.

Potential branch:
`decision_ledger_020/branches/steam_root/`

Open when glossary migration is either complete enough or explicitly paused.

========================================================================
4. Toolchain Portability And Targets
========================================================================

Purpose:
Resolve the Rustup-vs-standalone archive question from evidence and define the host/target model.

Inputs:

- Steam root package policy.
- Workspace hermeticity expectations.

Current code facts:

- SDK wraps Rustup with Vapor-owned `RUSTUP_HOME` and `CARGO_HOME`.
- SDK prefers app-local `rustup/bin/rustup`, but currently may use PATH Rustup.
- Current supported target model names Linux GNU and Windows MSVC.

Unresolved decisions to collapse later:

- Exact relocation test.
- Whether PATH Rustup is development-bootstrap only.
- Whether `toolchain install` should become idempotent.
- Whether Windows GNU becomes first cross-build proof.
- Whether host triples, stdlib targets, linker targets, and release targets split into separate concepts.

Potential branch:
`decision_ledger_020/branches/toolchain_portability/`

Open after the glossary migration plan unless relocation testing becomes urgent enough to branch in parallel.

========================================================================
5. Workspace Execution And Hermeticity
========================================================================

Purpose:
Define what Vapor-managed Cargo commands actually guarantee.

Inputs:

- Toolchain portability.
- Repository topology.

Current code facts:

- Vapor-managed Cargo commands write output under `$VAPOR_HOME/output/dev/<workspace>`.
- Existing PATH is preserved after Vapor path prefixes.
- `RUSTC_WRAPPER` is removed, but many other env vars are not scrubbed.

Unresolved decisions to collapse later:

- Whether preserving PATH is compatible with "no system leaks".
- Which env vars must be scrubbed for a hermeticity claim.
- Whether `workspace sync` and `sdk fmt` should prompt.
- Whether `workspace deploy` is public SDK command or internal root maintenance.

Potential branch:
`decision_ledger_020/branches/workspace_hermeticity/`

========================================================================
6. Manifest, Lockfile, Artifact, Fingerprint
========================================================================

Purpose:
Give active code a small, non-aspirational data model for Vapor.toml, future Vapor.lock, artifacts, and fingerprints.

Inputs:

- Documentation substrate.
- Packs/capability scope.
- Steam root and Workshop scope.

Current code facts:

- `Vapor.toml` exists in active repos with workspace kind/id, root toolchain pin, registry metadata, and custom-content graph.
- No `Vapor.lock` implementation exists.
- No fingerprint implementation exists.

Unresolved decisions to collapse later:

- Minimal active schema.
- First lock-like artifact.
- First fingerprint form.
- Artifact terms safe for active code.

Potential branch:
`decision_ledger_020/branches/manifest_lock_artifact/`

========================================================================
7. Packs, Capability, Rhai, Runtime Concepts
========================================================================

Purpose:
Keep the broad conceptual model from swallowing the bootstrap. Only introduce runtime/capability/Rhai terms when they are needed by code or canonical glossary migration.

Inputs:

- Canonical glossary migration.
- Manifest/fingerprint model.

Known posture:

- Packagepack/Enginepack/Gamepack/Modpack are comparatively stable.
- Capability/Rhai/Runtime Lock vocabulary still needs careful handling.
- Rhai validation is not the immediate active work.

Unresolved decisions to collapse later:

- First pack validation primitive.
- Whether current engine/game examples are generic placeholders.
- When reserved names `core_engine`, `core_mod`, `base_mod` become enforced.
- Smallest capability concept worth implementing.

Potential branch:
`decision_ledger_020/branches/packs_capability_rhai/`

========================================================================
8. Steam Workshop, Registry, Trust, Install Ledger
========================================================================

Purpose:
Separate root SteamPipe publishing from public Workshop/content distribution and avoid false security claims.

Inputs:

- Steam root/self-bootstrap.
- Manifest/fingerprint model.
- Repository topology.

Current code facts:

- SDK has SteamCMD status/login/run_app_build.
- SDK root publish does not enforce Vapor-Registry permissions.
- No Workshop implementation exists.

Known posture:

- Steam is required for public/published Vapor usage.
- Local/offline authoring remains possible.
- "Trusted but validated for compatibility/integrity" is safer than hostile-code sandbox wording.
- Signing is not Phase 3 active scope.

Potential branch:
`decision_ledger_020/branches/workshop_registry_trust/`

========================================================================
9. Command Surface And Safety
========================================================================

Purpose:
Keep SDK/Launcher command vocabulary useful without pretending stubs are stable public contracts.

Inputs:

- Documentation authority.
- Steam root/toolchain/workspace behavior.

Current code facts:

- SDK exposes broad command stubs plus real environment/toolchain/workspace/root/steam slices.
- Launcher is mostly stubbed.
- CLI `CommandSpec` text is documentation/policy surface and requires approval for semantic edits.

Potential branch:
`decision_ledger_020/branches/command_surface_safety/`

========================================================================
10. Code Cleanup And Maintainability
========================================================================

Purpose:
Make code more maintainable without smuggling policy decisions into names, comments, rustdoc, or command text.

Inputs:

- Relevant upstream decisions per module.

Candidate cleanup packets:

- Split `workspace/manage.rs`.
- Split `workspace/deploy.rs`.
- Split `toolchain.rs` after relocation test unless purely mechanical.
- Introduce shared SDK app-root layout only after naming/layout policy is accepted.
- Keep SDK/Launcher safety/spec duplication until semantics stabilize.

Potential branch:
`decision_ledger_020/branches/code_cleanup/`

========================================================================
Branching Policy
========================================================================

Open a branch when:

- a topic would add more than a small handful of questions to the main trunk,
- an implementation packet needs inventory/planning,
- answers in the topic can invalidate downstream main-roadmap questions,
- or actual work needs to happen before the main trunk can continue.

Do not open a branch when:

- a question can be answered inline in the main trunk,
- the topic is only a small factual clarification,
- or the branch would only restate an existing branch.

Branch outputs should be one of:

- an approved implementation packet,
- an inventory/report,
- a rewritten branch queue,
- a concise branch summary for returning to the main trunk,
- or an explicit stop/defer decision.

========================================================================
Immediate Active Branch
========================================================================

Active branch:
`decision_ledger_020/branches/glossary_migration/`

Current branch file:
`020d-gm-a_queue.txt`

Main trunk pause point:
`020d_referee_queue.txt`, Pivot Check 1.

Return target:
After glossary migration branch convergence, write `020e_referee_queue.txt` with:

- a short branch summary,
- any accepted docs/roadmap/glossary structure,
- and a rewritten Active Repo Topology section that accounts for the canonical docs structure.

END OF 020d-RM-a
