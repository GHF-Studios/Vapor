# Vapor Agent Protocol

Purpose: govern AI work in the Vapor repo. This file is operating protocol, not product doctrine.

## Authority

1. The owner is the only semantic authority.
2. Owner answers in chat are high-authority raw input, not automatic doctrine.
3. Vapor is the root repo for the ecosystem layer. It defines generic Engines, Games, Mods, packs, SDK/launcher-facing protocol, capability/Rhai authoring, validation, and distribution concepts.
4. Vapor may name generic roles such as Engine, Game, Mod, Extension Mod, Enginepack, Gamepack, Modpack, and Packagepack.
5. Vapor must not depend on, name, or assume concrete Spacetime Engine or Loo Cast Game internals.

## Default Mode

1. Questions are the default work mode.
2. For broad uncertainty, use question batches.
3. Do not rush synthesis, implementation, or documentation.
4. You may suggest that action may now be useful, but do not perform it without explicit approval.

## Approval Rules

1. Do not edit files, create files, generate code, write docs, run formatters, run tests, or run validation unless explicitly approved.
2. If approved to propose implementation, present the substance first: repo, file list, and exact intended content shape.
3. Do not explain by default. Present and consult on substance; clarify only when asked or needed.
4. Stop and ask if an approved task becomes underspecified.

## Repo Boundaries

Current repo stack:

```text
Vapor <- Spacetime-Engine <- Loo-Cast-Game
```

Satellite repos:

- `Vapor-SDK`: authoring, project/template/toolchain, validation, publishing, and distribution workflows.
- `Vapor-Launcher`: install, select, validate, compose, and launch workflows for players and modpack authors.
- `Vapor-Examples`: official example/test content consuming the stack like an external project.

## Dependency Rules

1. Development defaults to Git/branch dependencies when useful.
2. Release defaults to crates.io versions and Steam/proper distributed artifacts.
3. Distributed content/artifact dependencies are separate from Rust crate dependencies.

## Docs Rules

1. Do not create broad docs surfaces by default.
2. Prefer root `README.md`, crate rustdoc, module-local docs, and clear local names for implemented behavior.
3. `docs/roadmap/` is the accepted roadmap hierarchy location.
4. `docs/books/` is the accepted location for mdBook stubs and future book drafts.
5. `docs/glossary/` and `docs/tech_glossary/` are accepted destination concepts for the planned selective glossary migration, but glossary files still require an approved migration packet.
6. TOML comments should be rare and local.
7. Do not create diagrams, RFCs, or unrelated docs pages unless explicitly requested.
8. Design intent, doctrine, roadmap, naming policy, workflow policy, and public command semantics require owner discussion before being written as stable documentation, rustdoc, comments, or CLI text.
9. Old `Loo-Cast` glossary material is source material for canonical Vapor glossary work after migration. Old RFCs, diagrams, and unrelated root docs remain quarry/archeology unless separately approved.

## Question Batches

1. Use the original plaintext format: title, optional note/context, numbered questions, `A:` answers.
2. Preserve raw owner answers, including informal language, uncertainty, profanity, and side comments.
3. Store one batch per file under `ai_conversation_logs/`.
4. Preserve numbering from the wider project history.
5. Chat batches may include a short "what this unlocks" note.
