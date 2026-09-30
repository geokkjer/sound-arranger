# Learn Rust with sound-arranger

> 🕒 Last verified against commit `2f6ad78` (2026-09-30). If the code has moved on,
> trust the code and move this line forward.

A mini course that teaches Rust by reading and modifying *this* codebase. You
said you want to understand the AI-written Rust here — so instead of toy
examples, every lesson takes a real file, explains it line by line at the level
of "why is this written this way", and ends with exercises that touch that file
or something shaped like it.

Pair it with [the architecture explainer](../architecture-explainer.md), which
covers the *audio engineering* why; this course covers the *Rust language* how.
If you haven't operated anything yet, do [the first-session tour](../FIRST_SESSION.md)
first — fifteen minutes, no prerequisites.

## What you need

- **Rust** via [rustup](https://rustup.rs). This repo uses edition 2024 and
  let-chains, so use a recent toolchain: `rustup update`.
- **An editor with rust-analyzer** (VS Code + the rust-analyzer extension is
  fine). Inlay hints for types are your best friend while learning.
- That's it. No external crates beyond `cpal`, `serde` and a git-pinned
  `midir` (MIDI output) in `media/` (all ignorable until Lesson 8); `engine`'s
  optional `fundsp` feature is off by default.

Verify your setup works:

```sh
cargo test -p engine        # runs the core tests, ~a second
printf 'host v1\nbounce 4800 /tmp/faucet.wav\n' | cargo run -p host
                            # the headless binary bounces a silent WAV from stdin
```

## How to work through it

Lessons are ordered so each one only needs concepts from earlier ones. The
golden rule: **don't just read — type.** For each lesson there's a "Your turn"
section with exercises. Do them for real; the tests already in the repo will
tell you when you're right.

A habit worth building from day one: whenever you wonder "what does this line
do?", hover it in the editor, then run `cargo test` after any change. The
compiler is the teacher; your job is to keep asking it questions.

## The lessons

| # | File | Teaches | Repo material |
|---|------|---------|---------------|
| 1 | [Structs, methods, ownership](01-structs-and-ownership.md) | structs, `impl`, `&self` vs `&mut self`, `Vec`, moving vs borrowing | `engine/src/clock.rs` |
| 2 | [Enums, match, Option & Result](02-enums-match-result.md) | sum types, exhaustive matching, error handling | `engine/src/log.rs`, `graph.rs` |
| 3 | [Traits and generics](03-traits-and-generics.md) | trait definitions, default methods, generics, trait objects | `graph.rs`, `plugins/mod.rs` |
| 4 | [Closures and the disposer pattern](04-closures-and-disposers.md) | closures, `Fn`/`FnMut`/`FnOnce`, capturing with `move` | `plugins/euclidean.rs` |
| 5 | [Lifetimes and shared borrows](05-lifetimes-and-borrowing.md) | lifetime annotations, multiple `&mut` into `self`, iterator lifetimes | `render.rs`, `clock.rs` |
| 6 | [The no-allocation render path](06-no-allocation.md) | const generics, slices, fixed buffers, `debug_assert!` | `graph.rs` (`EventBuf`) |
| 7 | [Threads, atomics, and a lock-free ring](07-concurrency.md) | `thread::spawn`, `Arc`, atomics, memory ordering, a peek at `unsafe` | `media/src/ring.rs` |
| 8 | [Modules, errors, and the whole program](08-modules-errors-host.md) | crate/module layout, visibility, error idioms, putting it together | `host/src/lib.rs` |
| 9 | [Paradigms and design principles](09-paradigms-and-principles.md) | functional / OO / procedural modes in Rust; SOLID, KISS, DRY, YAGNI as this codebase practices them | everything |
| 10 | [Refusals as data](10-refusals-as-data.md) | error enums designed for a path that cannot allocate; bounds that are capacity; one rule, two doorways | `render.rs` (`ApplyFault`), `graph.rs` (`ConnectClass`) |
| 11 | [Total functions and saturating arithmetic](11-total-functions-and-saturating-arithmetic.md) | total vs partial, `saturating_*`, float→int casts, refusing the unrepresentable | `clock.rs`, `render.rs` (`MIN_TEMPO_BPM`) |
| 12 | [Bounded walks and loud caps](12-bounded-walks-and-loud-caps.md) | no unbounded loop on the render thread; caps paired with counters; binary search on monotone lookups | `graph.rs`, `plugins/clock_out.rs`, `render.rs` (`drain`) |

Lessons 10–12 are the **hardening tier**, written from the 2026-09 review's
material: they teach the discipline the codebase learned the hard way, in the
code that learned it. Do them after 6, or whenever a "why is this written so
defensively?" question sends you here.

## The two ideas everything hangs on

If you internalize nothing else from either the code or the course:

1. **Time, graph, log — then plugins.** The core (`crates/engine`) knows about
   time, wiring, and events. Nothing else.
2. **The render path never allocates or blocks — and no loop on it runs
   unbounded.** This single rule explains nearly every "weird" Rust choice in
   the codebase — the fixed-capacity buffers, the atomics, the `unsafe` ring
   buffer, the closure-shaped APIs. Lessons 10–12 are the rule's finest print:
   refusals recorded as *data* (not formatted sentences), counters total and
   saturating (not panicking or wrapping), and every cap paired with a counter
   that says what the cap clipped.

## Reading order inside a lesson

Each lesson follows the same shape:

1. **Concept** — the language feature, minimal example.
2. **In the wild** — the real file, explained.
3. **Your turn** — exercises, smallest first. Exercises marked ⭐ have their
   answer checkable by running an existing test; ones marked 🔧 ask you to
   write new code.
4. **Checkpoints** — quick self-test questions (answers at the bottom).
