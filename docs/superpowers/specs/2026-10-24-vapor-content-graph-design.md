# Vapor Content Graph Design

## Intent

Define and implement the semantics and lifecycle of Vapor’s existing ten content kinds as one coherent authoring, resolution, build, distribution, installation, and runtime model. The vocabulary remains exactly: Packagepack, Enginepack, Gamepack, Modpack, Engine, Game, EngineMod, GameMod, ExtensionMod, and Crate. No kind is added or removed without a strong evidence-backed justification and explicit owner approval. Existing behavior is evidence to challenge, not a preservation constraint.

## Owner definitions and model

- **Packagepack** is the root of a playable selection ("composition" is an informal synonym). It encompasses the source needed to edit, modify, or fork the package and a fully compiled, linked, launchable executable. These are distinct source and runnable projections of the same authored package identity/release, not separate content kinds.
- **Enginepack** bundles an Engine with EngineMods and may expose optional extensions. A consuming Game may use none, some, or all optional additions; mandatory requirements remain explicit.
- **Gamepack** similarly bundles a Game with optional GameMods for downstream content to select.
- **Modpack** groups additions for a particular extended experience. Each of these packs may represent mutually exclusive paths; pack membership does not imply that every member is simultaneously compatible or selected.
- **Engine** and **EngineMod** provide reusable runtime/API/framework capability. **Game** builds on that capability to define what an experience does. **GameMod** adds gameplay-facing content/behavior. Both roles may use Rust; this split does not prescribe Rust versus Rhai.
- **ExtensionMod** extends one Mod, with a generic target kind so extensions can attach to EngineMods or GameMods and to further mods where valid.
- **Crate** is a Vapor-managed ordinary Rust crate that can be composed without requiring an Engine/Game stack.

## Graph semantics

Dependency edges must distinguish at least required target/role, required dependency, optional dependency/feature, selected alternative, compatibility constraint, conflict/exclusion, and build/link usage where those semantics differ. Target cardinality is independent of optionality. A Mod’s mandatory target does not imply that every dependency it has is mandatory, and a pack can contain alternatives without selecting all of them. Resolution must produce an explicit selected graph and explain omitted or rejected branches.

The authored source graph, build graph, runnable artifact set, and installed selection are related projections with explicit provenance; they must not be collapsed into one vague "dependency" relation. A selected installed composition is runtime/user state for a Packagepack release, not another authored content kind.

## Goals

- Make each existing kind earn its distinct semantic role and complete lifecycle.
- Support required, optional, alternative, compatibility, and conflict relationships without frontend-specific interpretations.
- Preserve Packagepack source/editability and runnable binary delivery under one understandable identity.
- Make Engine/Game/Mod/Crate build inputs and outputs discoverable and reproducible.
- Expose actionable resolution diagnostics and stable selection results to CLI, Launcher, Studio, IDE, Workshop, and Installer.
- Keep current alpha schema/version policy; do not introduce a version bump as a shortcut.

## Anti-goals

- Treating all pack members as jointly installable or compatible.
- Equating an informal "composition" with a new content kind.
- Treating a Rust crate, Vapor content artifact, source project, Workshop item, executable, and installed selection as synonyms.
- Encoding Engine/Game solely as Rust/Rhai language boundaries.
- Preserving a resolver, provider, manifest field, or UI concept merely because it currently works.
- Expanding this first content pass into the full UI toolkit, IDE, operation console, or Rhai-to-Rust/WASM implementation; record their required contracts for their own workstreams.

## Constraints and open design checks

- Inspect and challenge current dependency rules, including asymmetric fallback rules, pack cardinalities, ExtensionMod target rules, and candidate version selection.
- Define how Packagepack source closure and platform executable artifacts are related and obtained without requiring a player to compile Rust.
- Define how a Game selects optional members from an Enginepack and how higher-level content selects Gamepack members.
- Define how Modpack represents incompatible or mutually exclusive paths and what selection input is required to resolve one.
- Determine how a consumer can require an optional Mod feature and how conflicts are reported.
- Keep the schema compatible with the current alpha version policy unless an unavoidable change is explicitly approved.

## Completion evidence

The graph model has focused tests for all ten kinds, required targets, optional edges, alternative selections, compatibility, conflicts, cardinality, ExtensionMod chains, and Crate-only dependency constraints. At least one authored example demonstrates Packagepack source plus runnable artifact, Enginepack optional mods, Gamepack optional mods, a branching Modpack, a Create-like EngineMod/GameMod/ExtensionMod graph, and a standalone Crate. Managed Vapor validation and a target-matrix package/launch proof pass; diagnostics identify the exact edge and selection that failed.
