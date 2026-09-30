# Agent Note: `frame_at` is total — the sum saturates too

Status: implemented

## Problem

The [tick-walk note](./2026-09-29-the-tick-walk-is-bounded-not-just-the-scratch.md) made
`TempoMap::frame_at` saturate: an unreachable beat answers `u64::MAX` — "past the end of time" —
instead of a real frame, so every frame-walking caller terminates on it. An independent verification of
that landed fix found the saturation is the *only* guarded part of the function, and that the
invariant the note states — *the render path terminates without panicking* — was contradicted twenty
lines below it:

```rust
return seg.start_frame + frames;   // crates/engine/src/clock.rs
```

The one integer sum on the whole path was unchecked. A segment that starts within one beat's worth of
frames of `u64::MAX` puts `start_frame + frames` past the top, and the beat domain that produces one
is ordinary: `Engine::seek` takes *any* `u64` frame, `Clock::push_tempo` appends a segment at the
frame the seek left the clock on, and one beat at 60 bpm is 48 000 frames. So `seek(u64::MAX - 4)`
followed by a tempo change is enough, and a render-path reader of the map — the clock-out node's tick
walk, the euclidean node's pulse placement — reads it.

Two consequences, and the second is the worse one:

- **In debug, a panic on the render thread.** Nothing catches it there; the actor thread is gone and
  the shell freezes.
- **In release, a wrapped frame.** `u64::MAX - 4 + 48_000` is `47_995` — frame 47 995, at the start of
  the session. A tick that should have been "past the end of time" is silently placed a third of a
  second in, which is the [step-walk note](./2026-09-29-the-euclidean-step-walk-is-bounded-and-counted.md)'s
  own subject from the other end: not a bounded walk, a wrong answer.

This is pre-existing rather than introduced by that fix — the function returned `0` there before it,
which was equally wrong — but the fix is what made the surrounding code load-bearing. The saturation
is now the function's documented contract, and an unchecked sum two lines under a documented
saturation is a claim the code does not keep.

`Clock::advance` is the same defect one layer down and the same trigger: `self.frame += frames`, with
the render loop calling it on every block. `Engine::seek(u64::MAX - 10)` and one 512-frame block is a
debug panic on the render thread, and in release a wrap to frame 0 that restarts the timeline from the
beginning in the middle of a render. Fixing `frame_at` and leaving it would have made the regression
test below a fiction — it passes only because it never renders a block.

## Decision

**`frame_at` is total in the frame domain, and the sum is the last unchecked thing in it.** The sum is
`saturating_add`, and the doc comment now states the whole contract rather than one arm of it.

- **The sum saturates.** A beat the map places inside a segment that starts near the top of the range
  answers `u64::MAX` — the same "past the end of time" the unreachable-beat arm already returned, so
  the two arms agree and there is one saturated answer, not two different kinds of wrong. Wrapping was
  never an option: it is not a saturation, it is a lie, and a `frame_at` that answers a beat before
  the frame it was reached from is a non-monotone function — which is precisely what the clock-out
  node's `due_ticks_from` binary search and its `tick_frame(u64::MAX) < frame ⇒ None` early exit both
  assume it is not.
- **The claim is total and monotone, and it is now written down where a caller reads it.** For any
  `f64` beat — negative, `NaN`, infinite, absurd — and any map whose segments ascend, `frame_at`
  answers with some `u64` and panics on nothing. That is three properties together, and each is
  load-bearing: the `f64` segment arithmetic cannot overflow; Rust's `f64 → u64` casts *saturate*
  rather than wrap, so a negative or infinite `frames` cannot become a huge frame; and the one integer
  sum is saturating. Monotonicity survives the saturation because **only the final open-ended segment
  can reach the sum at all** — a closed segment's `frames` never exceeds its own length, so its sum
  cannot pass the next segment's `start_frame`, and there is no later segment to answer lower after the
  last one. That is the whole argument, and it is why `saturating_add` is a saturation here and not a
  quiet break of the search's precondition.
- **`Clock::advance` saturates too**, for the same reason and by the same one-token change. Below the
  top of the range nothing moves; at it, the clock stops at `u64::MAX` instead of wrapping to frame 0.

## Alternatives considered

- **Return `Result<u64, …>` or panic deliberately on a frame the map cannot place.** Rejected, and the
  signature already argues it: `frame_at` returns `u64`, it is called from a render-path `while` in
  two nodes, and turning a saturating contract into a `Result` would push a `?` — or a
  `unwrap_or(u64::MAX)` that is the same saturation wearing a hat — onto every caller, including a
  TUI ruler computing a pixels-per-beat divisor. A panic on the render thread is the outcome the
  tick-walk note spent itself removing; reintroducing it deliberately, for a case the map can answer
  honestly, is a regression dressed as a decision.
- **Saturate the *beat* on the way in — refuse a seek or a tempo change within one beat of the top of
  the range.** Rejected: `Engine::seek` and `TempoMap::push` are public and total today, a caller
  placing the clock at `u64::MAX` is not making a mistake (a log written at a high frame, a replay
  onto a long session, an embedder's own timeline), and the honest answer to "where is this beat" when
  the answer is past the end of the range is `u64::MAX`. It would also need two new refusals to
  defend a one-operator change.
- **Clamp `frames` so the sum cannot overflow, instead of saturating the sum.** Rejected: clamping
  discards the distance and answers a *different* real frame just below the top of the range, which
  is the "falling back to a real frame" the tick-walk note explicitly rejected — a caller walking
  frames would then walk into the top of the range and keep going, which is the hang the saturation
  exists to end.
- **Fix `frame_at` and leave `Clock::advance` for a separate change.** Rejected as splitting a fix
  that does not work: the trigger is the same seek, and the regression test this change exists to add
  is only meaningful if the clock can actually survive the seek. It is called out in Consequences as
  the one thing here beyond the reported line, so a reviewer who wants it split has a clean seam.
- **Audit every unchecked integer in the engine's render path while the file is open.** Rejected as
  scope, deliberately: the reported finding is one line, and the audit that *is* in scope is
  `TempoMap`'s own arithmetic. The neighbouring unchecked adds that the audit turned up are listed
  in Consequences as **found and not fixed**, with their reachability, so the next slice can take them
  as a set rather than rediscovering them.
- **Harden `beat_at`'s and `frame_at`'s segment-length subtractions with `saturating_sub` while
  here.** Rejected, and this one is a considered "no" rather than a deferral: those two subtract a
  segment's start from its own end, and under `push`'s documented ascending contract the end is never
  below the start, so they cannot underflow. The only way to reach them is a `push` that violates that
  contract — and `push` states it, with a `debug_assert!` that fires **in the same build that would
  otherwise panic**, on the control side where a loud panic is the right place for a caller bug. In
  release the assert is inert and the subtraction wraps silently, which is a wrong answer rather than
  a panic, and it is the [step-walk note](./2026-09-29-the-euclidean-step-walk-is-bounded-and-counted.md)'s
  already-recorded statement that a hand-built map may be non-monotone — a property the capped walks
  already tolerate. A silent wrong value behind a documented, asserted contract is a smaller problem
  than the panic that was actually reported, and hardening it would have meant changing behaviour for
  out-of-contract input as a side effect of a different fix.

## Consequences

- Rendered: `frame_at`'s sum is `saturating_add` and its doc states totality and monotonicity;
  `Clock::advance` is `saturating_add`. Both are behaviour-identical below `u64::MAX` — a 120 bpm
  session, a 48 kHz hour, everything the suite covers.
- Tests, in `crates/engine/src/clock.rs`'s `mod tests` beside the other `TempoMap` tests:
  - `frame_at_saturates_the_sum_when_a_seek_puts_a_tempo_near_the_top` — **the regression test.** The
    verifier's trigger verbatim: seek to `u64::MAX - 4`, `push_tempo(60.0, 4)`, then `frame_at` a beat
    past it. It asserts the saturation, monotonicity across a ladder of beats, that no answer falls
    below the segment it was placed in, and the round trip a wrapped frame breaks. On the unfixed code
    it **panics in debug** (`attempt to add with overflow` at the reported line) and in **release
    fails with `left: 47995, right: 18446744073709551615`** — the wrap, named.
  - `advance_saturates_at_the_top_of_the_frame_range` — the same seek and one block's `advance`.
    Panics in debug on the unfixed `+=`.
  - `frame_at_is_total_over_the_beat_domain` — the totality claim tested rather than asserted:
    `NaN`, `±inf`, negative and absurd beats, and a map with a zero, a negative and a `NaN` tempo,
    which can place nothing at all and must still answer. **This one passes on the unfixed code too** —
    it covers the `f64` domain the old code already got right, and is the companion to the sum test,
    not a second regression for it. It is here because the note asserts totality, and a claim in a doc
    comment with no test behind it is the thing this note is about.
- `cargo test -p engine` is green in both profiles: **122 tests, two of them doctests**, up from 119 —
  the 46 lib tests are 49, and the three added are all in `clock::tests`. The counting-allocator tests
  (`render_path_does_not_allocate`, `render_path_does_not_allocate_with_latency`) are unaffected —
  nothing here allocates.
- **Found by the same audit, and NOT fixed** (each is outside `TempoMap`, is a one-token
  `saturating_add`, and wants its own test in its own module):
  - `crates/engine/src/plugins/clock_out.rs`, `ClockOutNode::render` — `let block_end = block.frame +
    io.frames as u64;` runs on **every** block, so a block that straddles the top of the range panics
    there first, before `frame_at` is ever asked. This is the one that would keep the render thread
    dead after the very seek this change makes survivable.
  - `crates/engine/src/graph.rs`, `EuclideanGen::render` — `frame < block.frame + len` on the pulse
    placement line, reached on any pattern-true step.
  - `crates/engine/src/render.rs` and `crates/engine/src/plugins/clock_out.rs` — the host-side
    `advance` callers are already covered by the `Clock::advance` change above; the two `block.frame +
    len` sums above are not, and are the reason the "no panic after a seek near `u64::MAX`" property
    does not yet hold end to end.
- **A non-monotone map is still constructible** and is still a lie rather than a crash: `TempoMap::push`
  is public and takes any `f64`, so a negative or `NaN` bpm makes `beat_at` decrease, and a zero bpm
  makes `frame_at(0.0)` answer `u64::MAX` rather than 0. Nothing here panics — that is what the caps
  in the two sibling notes are for — but "the frame domain is total" means *total*, not *correct*:
  the contract is stated for a map whose segments ascend, which is `push`'s own precondition.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
