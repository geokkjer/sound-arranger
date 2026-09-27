# Lesson 8 — Modules, errors, and the whole program

**Material:** [`crates/host/src/lib.rs`](../../crates/host/src/lib.rs) — the
headless host: `parse_script` + `run_script`, which is the entire product in
miniature.

Last lesson: how a real program is *organized*. The host crate is the right
capstone because it touches every layer below it and demonstrates the idioms
that make a Rust codebase navigable: modules and visibility, closed
vocabularies, error formatting, and pure-function parsers.

## 8.1 Crates and modules in this workspace

The root `Cargo.toml` declares four members; each is a **crate** with its own
`Cargo.toml` (the fourth, `crates/workflow`, holds the key-driven editing model
the shells share). Inside a crate, files are **modules**:

```
engine/src/lib.rs      ← crate root: `pub mod clock;` etc. (you saw it)
engine/src/clock.rs    ← module clock
host/src/main.rs       ← binary target (the CLI)
host/src/lib.rs        ← library target (what tests and main use)
```

Rules worth knowing:

- `mod foo;` tells Rust to compile `foo.rs` (or `foo/mod.rs`) into the crate.
- `pub` makes items visible outside their module; `pub use` re-exports them at
  the crate root. Look at `engine/src/lib.rs`: it does both — declares modules,
  then flattens the good stuff (`pub use clock::{Clock, Scheduler, TempoMap};`)
  so callers write `use engine::*` instead of deep paths.
- A crate can have a library (`lib.rs`), binaries (`main.rs`, or
  `src/bin/*.rs`), tests (`tests/`), and inline `#[cfg(test)] mod tests` — all
  four exist here. Inline tests live next to the code they test;
  integration tests in `tests/` see only the public API.

## 8.2 The closed registry: interning text to `&'static str`

Lessons 2–3 noted that names throughout the engine are `&'static str`, which
seems to forbid user input. Here's the reconciliation:

```rust
pub const HOST_PLUGINS: &[&str] = &["euclidean", "scale", "tone", "mixer", "master"];

fn in_list(list: &'static [&str], s: &str, what: &str) -> Result<&'static str, String> {
    list.iter()
        .find(|n| **n == s)
        .copied()
        .ok_or_else(|| format!("unknown {what} '{s}' (registry: {})", list.join(", ")))
}
```

Read it as an idiom chain: search the static list → `.copied()` turns
`&&str` into `&str` (copying a reference is free) → `.ok_or_else(...)` converts
`None` into a formatted error **lazily** (the closure means the string is only
built when there's actually an error). User text never becomes a `&'static
str` by construction — it's *matched* against one. Closed vocabulary in,
garbage refused loudly out.

## 8.3 Errors without exception hierarchies

Notice every fallible function returns `Result<_, String>`. For a spike that's
fine and honest; for bigger programs you'd introduce a real error type. The
escalation ladder, all present somewhere in mainstream Rust:

1. `Result<T, String>` — quick, human-readable, no structure. (This repo.)
2. A dedicated enum error + `impl Display`, often via the `thiserror` derive.
3. `anyhow::Error` for applications — dynamic, with backtraces and context.

What matters for reading *this* codebase is the discipline around it rather
than the type: validate first (`validate_mount`, `validate_patch`),
return `Err` before any state change, and let `?` bubble messages up. Search
render.rs for `return Err(format!(` and you'll find the whole validation
surface readable as prose.

## 8.4 Pure parsing: `parse_script`

```rust
pub fn parse_script(text: &str) -> Result<Vec<HostCommand>, String>
```

A parser shaped as a pure function — text in, commands out, no I/O, no side
effects. That shape buys free testability (there are dedicated tests in
`host/tests/`) and reuse: today the CLI feeds it stdin, tomorrow an iced or
ratatui shell
feeds it validated frontend commands. Inside, it's plain token processing:
`line.split_whitespace()`, `.split_once('=')` for key=value pairs, then
`in_list`-style interning per slot. No regex, no parser-combinator framework —
just iteration and match.

Contrast with `run_script(script: &[HostCommand]) -> Result<HostSession,
String>`: execution is a separate step from parsing. Parse everything → then
run everything. This split is what lets the host guarantee "the same script on
fresh sessions produces byte-identical bounces": nothing in parsing depends on
state.

## 8.5 Reading a whole file flow

You now have every tool to read `host/src/lib.rs` top to bottom. The path a
script takes:

```
text ──parse_script──▶ Vec<HostCommand> ──run_script──▶ HostSession
                                                            │
                              engine.mount/patch/set_param ◀┘
                                        │
                        log + schedule (Lesson 5's two-phase pattern)
                                        │
                             Engine::render (Lesson 6's no-alloc loop)
                                        │
                        nodes: Sine/Gain/mixer/master/PlaybackNode (Lessons 3–4)
```

Every arrow is a concept you've studied. That's the course complete: you can
read any file in this repository and name the technique behind each line.

## Your turn

⭐ **1.** Run the host end-to-end with a script on stdin:

```sh
cat <<'EOF' | cargo run -p host
host v1
mount euclidean steps=8 pulses=3
mount scale
mount tone
mount mixer channels=2
patch euclidean.triggers scale.trigger
patch scale.note tone.note
patch tone.audio mixer.ch0
bounce 48000 /tmp/opencode/bounce.wav
EOF
```

Trace one command from the script through parse → apply → log entry. The
integration tests in `crates/host/tests/reference_host.rs` contain more script
examples (as inline strings).

🔧 **2.** Add a new command to the contract: `Marker { label }` that appends a
comment-ish event. Touch points: the `HostCommand` enum, `parse_script`,
`run_script`'s dispatcher, and one test. This single exercise rehearses the
whole lesson: enums, matching, validation, error strings.

🔧 **3.** Write three new cases for the script parser: unknown plugin, wrong
version line, malformed param assignment. Confirm each produces a *helpful*
error message — improving an error message is legitimate production work.

🔧 **4.** Reorganize something small: move `in_list` into its own module
(`host/src/names.rs` with `pub(crate) fn ...`). Experience visibility keywords
(`pub`, `pub(crate)`, private) doing their job across a file boundary.

## Checkpoints

1. Difference between `mod`, `pub mod`, and `pub use`?
2. Why is `parse_script` being a pure function valuable?
3. What does `.ok_or_else(|| format!(...))` buy over `.map_err(|_| format!(...))`
   on an `Option`?

*(Answers: 1 — `mod` declares/includes a module; `pub mod` also exposes its
existence publicly; `pub use` re-exports existing items under another path.
2 — testable without setup, reusable across frontends, order-independent.
3 — `ok_or_else` transforms `Option`→`Result` and builds the error lazily,
only when actually `None`.)*

---

## Where next

- Re-read [`../architecture-explainer.md`](../architecture-explainer.md) — it
  will read very differently now.
- The tests are specifications: `spike_a.rs` (determinism, allocation),
  `ring.rs` (concurrency), `reference_host.rs` (end-to-end).
- Build something small against the engine: mount plugins, patch them, render,
  assert on samples. Writing your own integration test in `crates/engine/tests/`
  is the best final exam there is.
