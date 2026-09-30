# Lesson 10 — Refusals as data: errors you can't afford to format

**Material:** `ApplyFault` in [`crates/engine/src/render.rs`](../../crates/engine/src/render.rs)
(lines ~129–245) and `ConnectClass`/`ConnectRefusal` in
[`crates/engine/src/graph.rs`](../../crates/engine/src/graph.rs), plus the
transactional `apply_mount` from Lesson 5.

Prerequisites: Lessons 2 (enums, `Result`), 5 (the `apply_mount` transaction),
and 6 (the no-allocation rule). This lesson is what happens when all three
collide.

## 10.1 The problem: an error you can't print

Every Rust course teaches error handling as `Result<T, E>` plus `?`. That's the
control side. But the render path has a rule (Lesson 6): **no allocation, no
blocking** — and in this codebase, "an error occurred" is something that happens
*on the render path* too. A scheduled mount or patch, stamped into the log
frames ago, comes due inside `render_block`. The plugin refuses it. Now:

- You can't `format!` a message — that allocates.
- You can't `debug_assert!` — release compiles it away, and this exact mistake
  (a refusal visible only in debug) shipped once: the 2026-08-30 release-silence
  bug in [the fundsp guide](../soft-synth-fundsp.md).
- You can't panic — never on a real-time thread.
- You can't *ignore* it — two things that ought to agree (the log and the
  graph) disagree, and the session is degraded whether or not anyone says so.

The answer built in the 2026-09 hardening pass: **record the refusal as data —
an `ApplyFault` — and build the sentence later, on the control side, where
allocating is allowed.**

## 10.2 The shape: a class and borrowed identities

Read the two kinds of answer in `ApplyFaultReason`, and notice how the shape
follows what the refuser can hand over *without allocating*:

```rust
pub enum ApplyFaultReason {
    /// A plugin's own refusal … moved, verbatim, never formatted here.
    Given(String),
    /// A cord the engine or the graph refused: a `&'static str` class and the
    /// cord exactly as the log spells it.
    Cord {
        class: &'static str,
        from: (&'static str, &'static str),
        to: (&'static str, &'static str),
    },
}
```

Two very different payload strategies in one enum:

- **`Cord`** holds zero owned bytes. The `class` is a `&'static str` from
  `ConnectClass::as_str` (a `const fn` — the strings are in the binary), and the
  identities are `&'static str` the *log already holds*: recording this fault
  copies nothing, moves nothing, allocates nothing.
- **`Given(String)`** looks like it breaks the rule — but read the doc: the
  plugin's `apply` is `Result<_, String>`, so a refusing plugin has *already
  built and paid for* its sentence before the engine saw it. The engine **moves**
  the string into the fault. One allocation exists, made by the plugin, and the
  engine declines to add a second. (This is the honest limit the code
  documentation admits: removing it means changing every `Plugin::apply`
  signature.)

The sentence is built by `ApplyFault::describe()`, on the caller's thread, when
a host reads the list — and its doc comment says the quiet part loud: *"
Allocating here is the point, not an accident: this is a read."*

## 10.3 The graph's half: `ConnectClass`, a `Copy` vocabulary

`Graph::connect` refuses eight ways, and each way is a variant of `ConnectClass`
— `UnknownFromNode`, `Backward`, `NoFromPort`, `Direction`, `KindMismatch`,
`ControlDriven`, `ChannelMismatch`, … Read its definition and notice what it
*doesn't* hold: no `String`, no `Vec`, no format args. It's `Copy`. That's what
makes it recordable from the audio thread.

Beside it, `ConnectRefusal<'a>` — the class plus the two port names the caller
passed, **borrowed from the call** (`'a`) — and the pair of methods that split
the concern:

- `connect` returns `Result<(), ConnectRefusal>` — **one rule, the loud door**,
  for the control side, where the `Display` impl formats a sentence.
- `try_connect` (used by the render path) records the refusal as a fault
  instead of formatting it.

One rule, two doorways, chosen by *where the caller runs*. That's the whole
pattern in a sentence.

## 10.4 The storage: a bound that is also the capacity

Where do faults live? In a `Vec<ApplyFault>` — but read `MAX_APPLY_FAULTS`:

```rust
pub const MAX_APPLY_FAULTS: usize = 64;
```

The doc explains the trick: **the storage is reserved up front** (`Engine::new`,
the control side), so the bound *is* the capacity. Recording a refusal on the
render path pushes into a `Vec` that already has room for it — a `push` below
capacity does not allocate. And when a session misbehaves enough to exceed 64
faults, the rest are *counted* in `apply_faults_dropped()` rather than silently
disappeared: "a session that refuses a thousand mounts shows a thousand
refusals without holding a thousand strings."

The `is_degraded()` method closes the loop: the session knows, at any instant,
whether every applied event agreed with the log — and a host can surface that
without reading the fault list at all.

## 10.5 The rest of the discipline (recap from Lesson 5)

The fault machinery is the *second* half of the story; the first half is that a
refusal leaves **no state behind**. `apply_mount` is a transaction: the name
reservation goes back, `undo_a_refused_apply(watermark, bus)` removes every node
the failed apply added, and the log event stands — a replay of the same log
refuses at the same frame and records the same fault. Determinism survives the
bug.

## Your turn

⭐ **1.** Read the test
`a_refused_cord_on_the_render_path_records_without_allocating`
(`crates/engine/tests/spike_a.rs`). It uses the counting allocator from Lesson 6
— but pointed at the *fault list*: it forces a render-path refusal and proves
the recording allocates nothing. Run it; then break it by changing
`ApplyFaultReason::Cord`'s class to a `format!`'d `String` and watch the
allocator catch you.

⭐ **2.** Read `apply_faults` tests in `crates/engine/tests/apply_refusals.rs`:
find the one that proves a *replay* records the same fault at the same frame.
That test is the determinism claim made executable.

🔧 **3.** Add a `ConnectClass` variant for "the cord already exists" and thread
it through `as_str`, `Display`, and the `try_connect` fault path. The compiler
will walk you through every site that must answer the new question — that's
exhaustiveness doing your review.

🔧 **4.** `ApplyFault::describe` builds a different sentence per `reason` kind.
Rewrite it as a `match` returning `Cow<'_, str>` so the `Given` case avoids its
`message.clone()` (borrowed when read-only). When does the `Cow` pay off, and
when is the clone actually fine?

## Checkpoints

1. Why does `ApplyFaultReason` have both a `String` variant and an all-`&'static`
   variant, instead of one `String` payload everywhere?
2. Why is `MAX_APPLY_FAULTS`'s `Vec` *reserved* in `Engine::new` rather than
   grown on demand?
3. What is the difference between `connect` and `try_connect`, and what chooses
   between them?
4. Why does the engine *move* the plugin's refusal string instead of formatting
   its own?

*(Answers: 1 — the record is written on the render path; a class + borrowed
identities allocates nothing, and the only owned case is a string the plugin
already paid for. 2 — a `push` below capacity doesn't allocate, which is what
makes render-path recording legal at all. 3 — same rule, two doorways: the loud
formatted `Result` for the control side, the allocation-free fault record for
the render path; the caller's thread chooses. 4 — one allocation already exists
(the plugin's); a second one would be the engine's, on the audio thread, and the
message would be a paraphrase of a sentence it already has.)*

Next: [Lesson 11 — Total functions: saturating arithmetic on the audio
path](11-total-functions-and-saturating-arithmetic.md)
