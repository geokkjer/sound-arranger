# Agent Note: Rust mini-course under docs/rust-course

Status: implemented

## Problem

The owner is not proficient in Rust, and the codebase's Rust was written by AI agents. Reading the architecture explainer alone does not build the language fluency needed to review, modify, and trust the code. The codebase itself is an unusually good teaching corpus — small crates, strict invariants, heavily tested, each file showcasing specific idioms (newtypes, sum types, trait objects, const generics, closures-as-disposers, disjoint borrows, atomics, `unsafe` with SAFETY comments) — but nothing maps those files to a learning path.

## Decision

Add **`docs/rust-course/`**: an eight-lesson mini course that teaches Rust *through this repository*. Each lesson takes one real file as its material, explains the language feature it demonstrates at beginner level (below the architecture explainer's assumed six months of Rust), quotes the actual code, and ends with exercises verified against existing repo tests where possible.

Lessons: 1 structs/ownership (`clock.rs`) · 2 enums/match/Option/Result (`log.rs`, `graph.rs`) · 3 traits/generics (`AudioNode`, `Plugin`) · 4 closures/disposers (`euclidean.rs`) · 5 lifetimes/disjoint borrows (`render.rs`) · 6 no-allocation render path (`EventBuf`, counting allocator) · 7 threads/atomics/unsafe (`media/src/ring.rs`) · 8 modules/errors/host (`host/src/lib.rs`) · 9 paradigms and design principles — Rust as multimodal (functional/OO/procedural per problem), with SOLID/KISS/DRY/YAGNI translated to this codebase's practice (open traits vs closed enums, trait contracts as LSP, three DI mechanisms). A README covers setup, ordering, and method; lesson 8 ends with an end-to-end script exercise through the headless host (verified: mount → patch → bounce runs).

The course deliberately complements rather than restates [the architecture explainer](../../../../docs/architecture-explainer.md): that document explains the audio-engineering *why*; this one teaches the Rust-language *how*, at a lower starting level.

## Alternatives considered

- **A generic "learn Rust" curriculum** (the Book, rustlings) — rejected as primary path: it would not reference this codebase, so the two-birds goal (understand *this* code while learning) is lost; the README links them as supplements.
- **Annotating the source with tutorial comments** — rejected: pollutes production code, duplicates what lessons do better, and fights the repo's comment discipline.
- **One giant document instead of eight files** — rejected: lessons must be worked through over multiple sittings, and per-lesson files keep exercise instructions next to their material.
- **Skipping the agent note** — not permitted by standing orders for a non-trivial docs addition.

## Consequences

- The lessons quote code at specific locations; refactors of `clock.rs`, `log.rs`, `render.rs`, `plugins/euclidean.rs`, `graph.rs`'s `EventBuf`, or `ring.rs` should check whether quoted snippets and line references drifted.
- Exercise claims were verified against the current tree (`cargo test -p engine clock`, `cargo test -p media ring`, the host stdin script); future toolchain or API changes may invalidate individual exercises cheaply — they are self-checking by design.
- The course assumes edition-2024 features (let-chains) are available, matching the repo's own requirement.
