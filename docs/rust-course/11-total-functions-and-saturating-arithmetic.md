# Lesson 11 — Total functions: saturating arithmetic on the audio path

**Material:** [`crates/engine/src/clock.rs`](../../crates/engine/src/clock.rs)
(`advance`, `seek_to`, `TempoMap::frame_at`, and the tests at the bottom of the
file) and `MIN_TEMPO_BPM` in
[`crates/engine/src/render.rs`](../../crates/engine/src/render.rs).

Prerequisites: Lesson 1 (`Clock`, `TempoMap`). This lesson is why one line of
Lesson 1 changed during the hardening pass — and what "total function" means
when the input is *any `u64` a seek can name*.

## 11.1 Partial vs total, in one line each

A **partial** function has inputs it doesn't answer for. `u64` addition is
partial: at the top of the range it *panics in debug* and *wraps in release*.
Both are answers you didn't want, and they disagree with each other.

A **total** function answers every input with something defined and sane.
`saturating_add` is total: past the top it sticks at `u64::MAX`.

In most code, the difference is a curiosity. On the render path it's the
difference between a session that plays and one that, in the shipped release
build, silently restarts its timeline from frame 0 mid-block. That is not a
hypothetical — it's the documented reason `advance` is written:

```rust
pub fn advance(&mut self, frames: u64) {
    self.frame = self.frame.saturating_add(frames);
}
```

Read the doc comment above it in the file. It does the reasoning you should
copy: **who can put the counter near the top?** `seek_to` places the clock at
any `u64`, and the render loop advances every block. So the dangerous input is
reachable, in release, from a public API. Saturate.

## 11.2 `frame_at`: totality is a *proved* property, and saturation preserves the invariant

`TempoMap::frame_at(beat) -> u64` maps a beat to the frame it lands on. Its
doc comment claims two things: **total** (every beat answers) and **monotone**
(beats never map backwards). The hardening pass earned both. Look at the
saturation inside:

```rust
// Saturating, because a segment can start within one beat's worth
// of frames of the top of the range — `Engine::seek` takes any
// `u64`, and `push_tempo` appends a segment at the frame it left
// the clock on. Unchecked, a beat inside that segment is a debug
// panic on the render thread and, in release, a frame near zero: a
// tick moved back to the start of the session. Only the final
// open-ended segment can reach the sum at all (a closed segment's
// `frames` never exceeds its own length), so saturating cannot
// cost the monotonicity the doc above claims.
return seg.start_frame.saturating_add(frames);
```

Three moves worth memorizing:

1. **Name the reachable danger** — a tempo segment starting near `u64::MAX`
   comes from `seek` + `push_tempo`, both public.
2. **Choose saturation over checking** — `checked_add` would make `frame_at`
   return `Option<u64>` and push the partiality onto every caller, including
   the render loop. Saturation keeps the signature total.
3. **Prove the invariant survives** — the last sentence is the subtle one:
   saturation *could* break monotonicity (two beats mapping to the same
   saturated frame). It doesn't, *because only the final open-ended segment can
   reach the sum* — a closed segment's own length bounds it. The argument is
   written where the code is, and a test pins it
   (`frame_at_is_total_over_the_beat_domain`, clock.rs — go read it; it beats
   the domain with absurd beats and asserts total + monotone together).

That last style point is the lesson inside the lesson: **a `saturating_*` call
without a comment saying why saturation is *sound here* is half-done.** The
codebase treats saturation as a decision, not a reflex.

## 11.3 Floats in, integers out: the cast family

Beats are `f64`; frames are `u64`. The bridge is `as` casts — and in Rust,
**float→int `as` casts saturate** (since 1.45): `1e300 as u64` is `u64::MAX`,
`(-1.0) as u64` is 0, `f64::NAN as u64` is 0. That's total by construction, and
the euclidean walk (Lesson 12) leans on it:

```rust
let s0 = (b0 / step_beats).floor() as i64;
let s1 = (b1 / step_beats).ceil() as i64;
// The `f64 → i64` casts above saturate (they do not wrap), so at
// an absurd tempo this is honestly "as many as there are" rather
// than a negative or a wrapped one …
let grid = s1.saturating_sub(s0).max(0) as u64;
```

Notice the order of operations: saturate at the cast, then `saturating_sub`
(signed subtraction of two saturated values can still overflow), then `max(0)`.
Each step narrows the space of possible lies, and the comment says what the
final number *means* ("as many as there are") rather than pretending precision.

The integer→integer side is different: `u64 as u64` never lies, but
`i64 as u32` truncates and `u64 + u64` overflows. The rule this codebase
follows: **cast at the boundary (with saturation), compute in the wide type,
and narrow last.**

## 11.4 The other direction: refuse what you cannot represent

Saturation handles "the answer exists but overflows." A different failure is
"the input admits no honest answer at all." `Engine::set_tempo` meets one: at a
tempo slow enough, one 24-PPQN MIDI tick spans more frames than `u64` holds,
so every tick after the first resolves to the same frame — the clock can't
place it. Read `MIN_TEMPO_BPM`'s doc (render.rs): the floor is set **1.5e11×
above the mathematical break point**, the arithmetic that locates the break is
written out, and the choice is explicit:

```rust
pub const MIN_TEMPO_BPM: f64 = 1e-3;
```

…a quarter note lasting 16.7 hours — nothing musical is that slow, so the
refusal excludes only nonsense. **Saturate what you can represent; refuse what
you cannot; write down which is which.** Between them, `set_tempo(1e-15, 4)` —
the input that once hung the render thread forever (Lesson 12) — is now refused
at the door with a message instead of answered with a wedge.

## Your turn

⭐ **1.** Run the two property tests and read them as claims:

```sh
cargo test -p engine frame_at_is_total_over_the_beat_domain
cargo test -p engine advance_saturates_at_the_top_of_the_frame_range
```

Then break the property: change `advance` back to `+=`, run the second test,
and read the failure. (The first test still passes — why? What does that tell
you about which test owns which claim?)

⭐ **2.** In a scratch test, evaluate `(1e300f64).round() as u64`,
`(-1.0f64) as u64`, and `f64::NAN as u64`. Predict first, then run. These three
answers are the entire float→int saturation contract.

🔧 **3.** `TempoMap::beat_at(frame) -> f64` is `frame_at`'s inverse. Its input
`frame` comes from `u64`-land; its arithmetic divides by segment lengths. Is
*it* total? Find the input that stresses it, then either prove it's already
safe (write the test) or fix it the way the file already does.

🔧 **4.** Write `fn span_frames(start: u64, len: u64) -> u64` that returns
`start + len` without ever panicking, wrapping, or lying about the result at
the top of the range — and a test that checks the three regimes (middle,
exactly-at-top, past-top). Then compare with `end()` in
`crates/media/src/timeline.rs` and see which regime *it* had to choose.

## Checkpoints

1. What makes a function *total*, and why does the render path care more than
   typical application code?
2. When is `saturating_add` the wrong choice, and what's the alternative?
3. Why does `f64 as u64` never panic, and what are the three special answers?
4. Why is `MIN_TEMPO_BPM` a *refusal* rather than a saturation?

*(Answers: 1 — every input has a defined, sane output; a release build on the
audio thread has no panic handler and no unwinding budget, so a partial
function's failure mode ships silently. 2 — when the caller can meaningfully
recover from "this cannot be answered", `checked_add` returning `Option` pushes
the decision to the only code that knows what to do; saturation hides the
question. 3 — float→int casts saturate by language definition since 1.45:
too-big pins at the type's max, negative pins at 0 (for unsigned), NaN pins at
0. 4 — a tempo below the floor makes the tick *unrepresentable* (every tick
maps to one frame), so no saturable answer exists; the honest total function
is one that refuses the input at the API boundary.)*

Next: [Lesson 12 — Bounded walks and loud caps](12-bounded-walks-and-loud-caps.md)
