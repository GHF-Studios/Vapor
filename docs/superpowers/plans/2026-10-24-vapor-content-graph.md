# Vapor Content Graph Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Vapor’s existing ten content kinds resolve through a coherent source, selection, build, distribution, and install graph without adding or removing kinds.

**Architecture:** Keep content identity/type separate from source projects, Cargo packages, selected alternatives, build outputs, and installed user state. Replace loosely interpreted dependency edges with typed, owner-defined relationship semantics, then project the selected graph into authoring, build, and runnable artifacts.

**Tech Stack:** Vapor Rust content/domain crates, TOML manifests, managed Vapor validation, Rust unit/integration tests.

**Spec:** `Vapor/docs/superpowers/specs/2026-10-24-vapor-content-graph-design.md`

## Global Constraints

- Content vocabulary remains exactly the existing ten kinds; additions/removals require a strong justification and explicit owner approval.
- Keep the current alpha schema/version policy; no version bump without approval.
- A Packagepack has editable source and a fully compiled, linked runnable artifact.
- Pack members can be alternatives and need not all be compatible or selected together.
- Dependency target cardinality and mandatory/optional status are independent.
- Existing code and behavior may be redesigned or removed when evidence shows the semantics are wrong or redundant.
- Run project validation/builds only through Vapor-managed commands at the owning workspace root; never raw Cargo.
- Preserve pre-existing dirty changes, including Vapor-Installer and Loo-Cast Cargo.lock; do not edit Loo-Cast during this content-domain tranche.

## Review Focus

- A pack with mutually exclusive branches must resolve only the selected branch and report missing selection; cover in Task 3.
- A mandatory Mod target may coexist with optional feature dependencies; cover in Task 2.
- An ExtensionMod may target either EngineMod or GameMod and possibly another valid ExtensionMod; cover in Task 2.
- A Packagepack source release must remain editable while its runtime release launches without a player-side Rust build; cover in Task 4.
- A standalone Crate must resolve/build without requiring Engine or Game; cover in Task 4.

---

### Task 1: Specify the graph relationship vocabulary

**Files:**
- Modify: `Vapor/crates/vapor_content/src/content.rs`
- Modify: `Vapor/crates/vapor_content/src/authored_manifest.rs`
- Test: `Vapor/crates/vapor_content/src/content.rs` unit tests

**Interfaces:**
- Consumes: existing `ContentKind`, `DependencySpec`, authored edges, and pack selection groups.
- Produces: one canonical relationship classification used by every resolver/provider; exact Rust names are selected after inspecting manifest schema/provider call sites and must not duplicate frontend policy.

- [ ] Add failing cases for every current kind’s valid/invalid edge types, distinguishing required target, required dependency, optional dependency, and pack alternative membership.
- [ ] Run focused managed content-domain validation and confirm the new cases expose current ambiguities/asymmetries.
- [ ] Define typed edge/selection semantics in the shared Vapor content model; keep existing kind identities and spellings.
- [ ] Run focused managed validation; confirm old behavior is retained only where it matches the design.

### Task 2: Encode compatibility, conflicts, target cardinality, and optional selection

**Files:**
- Modify: `Vapor/crates/vapor_content/src/content.rs`
- Modify: `Vapor/crates/vapor_content/src/authored_manifest.rs`
- Modify: `Vapor/crates/vapor_content/src/resolution.rs`
- Test: `Vapor/crates/vapor_content/src/resolution.rs` tests

**Interfaces:**
- Consumes: Task 1’s relationship classification.
- Produces: deterministic resolution inputs and diagnostics for requested features, alternatives, compatibility, conflicts, and target cardinality.

- [ ] Add failing graph cases for required Engine/Game targets, optional EngineMod/GameMod use, selected alternatives, explicit conflicts, ExtensionMod target variants, and unsatisfied requirements.
- [ ] Run focused managed content-domain tests and confirm failures identify absent graph behavior.
- [ ] Implement selection-aware graph validation/resolution; produce stable selected edges and explicit excluded/unselected alternatives.
- [ ] Run focused managed tests and confirm diagnostics identify the exact content edge and failed constraint.

### Task 3: Make packs model bundled paths instead of flat child closure

**Files:**
- Modify: pack manifest schema/parser and graph resolution modules identified by Task 1 call-site audit
- Test: pack resolution tests in `Vapor/crates/vapor_content`

**Interfaces:**
- Consumes: Tasks 1–2 relationship and constraint semantics.
- Produces: resolved pack selection for Packagepack, Enginepack, Gamepack, and Modpack, including mutually exclusive branches.

- [ ] Add failing cases for an Enginepack with selectable EngineMods, a Gamepack with selectable GameMods, and a Modpack with mutually exclusive paths.
- [ ] Run focused managed tests and verify flat transitive closure incorrectly includes conflicts or cannot identify the required selection.
- [ ] Implement pack membership/selection projection without changing the ten-kind vocabulary.
- [ ] Run focused managed tests; verify each pack resolves a deterministic selected path and refuses an ambiguous required choice.

### Task 4: Connect source/editable and runnable Packagepack artifacts

**Files:**
- Modify: `Vapor/crates/vapor_content/src/resolution.rs` and package artifact planning modules discovered in repository audit
- Modify: relevant package/build manifest models under `Vapor/crates/`
- Test: Packagepack artifact planning tests and package staging fixture tests

**Interfaces:**
- Consumes: selected resolved Packagepack graph from Tasks 1–3.
- Produces: explicit editable source closure and target-specific linked executable artifact set with provenance to the same Packagepack release.

- [ ] Add failing tests that require source/editing provenance and a runnable artifact for each declared target, without compiling on a player install path.
- [ ] Run focused managed tests and verify current metadata-only assumptions fail the authored requirement.
- [ ] Implement separate source and runtime artifact projections while retaining one Packagepack identity/release.
- [ ] Run focused tests for Linux and Windows target staging, source availability, and executable launch metadata.

### Task 5: Prove Engine/Game/Mod/Crate composition and publishable examples

**Files:**
- Modify: canonical templates and example content manifests under `Vapor-Examples` only if included in this workstream’s owned scope; otherwise create fixtures in Vapor tests
- Test: end-to-end graph fixtures under `Vapor/crates/vapor_content` and package provider tests

**Interfaces:**
- Consumes: Tasks 1–4 shared graph and artifact plans.
- Produces: representative resolved graphs for all ten kinds and a standalone Crate build path.

- [ ] Add fixtures for standalone Crate, Engine→Crate, Game→Engine+Crate, Enginepack/Gamepack optional mods, Modpack variants, and Create-like EngineMod+GameMod+ExtensionMod chains.
- [ ] Run focused managed validation and verify each fixture matches its declared role and selected path.
- [ ] Update templates/docs to describe the validated semantics and Packagepack source/runtime projections.
- [ ] Run Vapor-managed validation and the required target-matrix build/package proof; record any remaining unresolved rule as a specific follow-up rather than silently broadening the tranche.
