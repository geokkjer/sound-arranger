# Agent Note: The euclidean step walk is bounded, and counted

Status: implemented

## Problem

The [tick-walk note](./2026-09-29-the-tick-walk-is-bounded-not-just-the-scratch.md) closed the
clock-out node's unbounded tick walk, and that commit's message claimed "no tempo can hang the
render". An independent review of the commit found the same shape **one node over**, in
`EuclideanGen::render` (`crates/engine/src/graph.rs`), and left uncapped:

```rust
let b1 = block.tempo.beat_at(block.frame + len.saturating_sub(1)) + step_beats;
let s1 = (b1 / step_beats).ceil() as i64;
for step in s0..s1 {
```

The walk's length is set by `beat_at` and by nothing else. At 1e300 bpm, 48 kHz, a 512-frame block
and the default `pulses_per_beat = 4`, the block's last beat is 1.77e296 beats, the `f64 → i64` cast
of the step index saturates at `i64::MAX`, and `s0` is 0 — so the **first block at frame 0** walked
9.2e18 steps. At a merely absurd 1e9 bpm the same block's grid is 709 724 steps, every block (the
review quoted 7.1e8 there, three orders high: 512 frames at 1e9 bpm is 177 430.556 beats, and a
quarter beat per step makes 709 724 of them).

It is reachable from a script: `euclidean` is in `HOST_PLUGINS`, `render_inner` renders every
mounted node, and `set_tempo` has no ceiling — `MIN_TEMPO_BPM` bounds the slow side only — so
`mount euclidean` plus `set_tempo 1000000000 4` is a permanent render-thread wedge: `render` never
returns, no further command is serviced, and the shell freezes. The sibling fix's `frame_at`
saturation cannot help here: the body `continue`s on the pattern check long before it calls
`frame_at`, and where it does call it, the cost is the iteration itself.

The commit that named the gap also did not close it — its message says the euclidean walk "is
bounded in the next commit". This is that commit. The claim is now true, and scoped below.

## Decision

**The block's grid is counted before it is walked, the walk is capped, and the remainder is
counted and published** — the clock-out node's three-part shape (`frame_at` saturates · the walk is
capped · the overflow is counted) applied to the node the first cut left behind.

- **The whole grid, in closed form, before any of it is walked.** `grid = s1.saturating_sub(s0)
  .max(0)`: how many steps a walk of `[s0, s1)` would visit, known without visiting any of them.
  Both `f64 → i64` casts saturate (Rust's float→int casts saturate; they never wrap), so at an
  absurd tempo this is honestly "as many as there are" rather than a negative or a wrapped number —
  which is what makes `grid - walked` **exactly** the count of what the walk skipped. No search, no
  second pass, no allocation.
- **The walk is capped at `EUCLIDEAN_STEP_CAP` = `BLOCK * 8` = 4096 steps** — on the *walk*, not
  only on the output, which is the distinction the sibling note is about. Eight steps per frame is
  where the grid becomes finer than the block's own resolution: `frame_at` rounds to whole frames,
  so from there on several steps share every frame and their offsets are indistinguishable. At the
  default four pulses per beat and a 512-frame block that is ~1.2e5 bpm (a sixteenth note every
  0.125 ms), and ~3e4 bpm at sixteen pulses per beat. Every tempo below the cap is walked whole, so
  the emitted triggers are unchanged for any tempo a person could mean, and `render` returns within
  4096 iterations for any tempo at all.
- **The walk also stops when the trigger buffer is full.** A push into a full `EventBuf` is refused
  anyway, so stopping there emits byte-identical triggers and only shortens the walk; it also means
  this node can no longer be the source of a silently refused push, which at a fast tempo it
  certainly was before (a dense pattern and 709 724 steps meant ~2.7e5 refused pushes per block). Both
  stops are counted under the same, honest heading: steps the block did not evaluate.
- **What the walk could not take is counted and published.** `EuclideanGen` holds an
  `Arc<AtomicU64>` read back by `drops()`; the `euclidean` plugin creates the counter, hands it to
  `EuclideanGen::with_drop_counter`, and provides it under **`euclidean.drops`**
  (`EUCLIDEAN_DROPS_KEY`), withdrawing it in the disposer — the `clock_out.overflows` pattern, so a
  host reads it the way it already reads `mixer.meters` and `clock_out.overflows`. The render path
  only ever `fetch_add`s, and only on a block that actually truncated, so a session at any musical
  tempo never touches the counter at all.
- **The index arithmetic is checked, as in the sibling fix.** The step advances by
  `saturating_add(1)` and the grid count is a `saturating_sub`: the loop condition is what stops the
  walk (`step < s1`, and `s1 ≤ i64::MAX`), not an increment running off the top of the range.
- **The node is built through a constructor, and that is where the length invariant lives.** The
  counter field is private, so `EuclideanGen::new` / `with_drop_counter` are the only ways in, and
  the constructor asserts `pattern.len() == steps.max(1)`: the pattern is indexed `step % steps`, so
  a hand-assembled mismatch is an out-of-bounds index **on the render thread**. This is a
  source-breaking change to a public struct, and the trade is stated under Consequences.

### Is "no tempo can hang the render" true now?

Yes, and here is exactly what that rests on: the tempo map is read on the render path in **two**
places — `ClockOutNode::render` and `EuclideanGen::render`; `rg 'beat_at|frame_at'` over `crates/`
finds those two plus the `TempoMap` implementation itself and doc prose. Both walks are now capped
and count what they skip, so no tempo makes any node's render loop unbounded.

What the claim does **not** cover, so that nobody reads it as more than it is: loops bounded by
inputs other than tempo (buffer sizes, note counts, block widths) were not re-audited here; a
hand-built `TempoMap` can still be non-monotone (`TempoMap::push` is public and takes any `f64`, so
a negative bpm makes `beat_at` decrease) — the cap tolerates that, because the walk stays bounded
whatever the map says, but it is why the bound is a walk and not a search; and whether an absurd
tempo should be *refused* at `set_tempo` rather than merely reported is the clock-out note's open
product question, unchanged here.

## Alternatives considered

- **Compute the step offsets in closed form — a binary search over the monotone `frame_at` — instead
  of walking.** Rejected as the whole fix: the range that *emits* is itself unbounded at an absurd
  tempo (every step rounds onto frame 0, so the emitting range is the whole saturated one), so a
  search would not remove the need for a cap, only move the hang to where the search brackets. It is
  also not exactly equivalent: it assumes `frame_at` is monotone in the step index, which
  `TempoMap::push` does not guarantee, and on a non-monotone map a search steps over an emitting
  step where the walk would have found it. It stays on the table as a way to *narrow* the walk in a
  later slice, never as the thing that bounds it.
- **Cap the walk at `CAP_EVENTS` (32), so the walk bound and the output bound are one number.**
  Rejected: it truncates at ~1.2e4 bpm (sixteen pulses per beat) — a grid that is still trivially
  cheap to walk — so it would change audible output where the old code was fine, and it conflates
  "how many onsets fit in a block" with "how much work a block may do".
- **Count the refused trigger pushes instead of the skipped steps.** Rejected: `EventBuf`'s capacity
  drop is documented behaviour and cannot tell a dense-but-sane pattern from a nonsense tempo, and it
  is not the thing that must be bounded — the walk is.
- **Refuse an absurd tempo at `set_tempo`, symmetric to `MIN_TEMPO_BPM`.** Rejected as the fix, for
  the reason the tick-walk note gives: `TempoMap::push` is public and the apply path pushes straight
  from the log without re-validating, so a door check is not the invariant. The policy question — a
  ceiling at all, and at what number — is untouched by this note.
- **A wall-clock deadline on the walk.** Rejected: rendering must stay a pure function of the log,
  and a time-based bound breaks `same log ⇒ byte-identical bounce` outright.
- **Keep the fields public and add a `pub drops`.** Rejected: a public field belongs in every
  caller's struct literal and so prevents no mismatch, while a private one makes the constructor the
  only way to build a node — which is the only reason the length assertion can mean anything.
- **Plumb the counter into the host snapshot's status line in this change.** Deferred on purpose: the
  context key *is* the engine's visibility mechanism, and adding a field to the host's status struct
  is a slice-B-shaped change with its own test. A host can read the count today with one
  `ctx.get::<Arc<AtomicU64>>(EUCLIDEAN_DROPS_KEY)`.

## Consequences

- `mount euclidean` plus `set_tempo 1000000000 4` — or 1e300 — renders and returns, and the walk it
  could not finish is visible in `euclidean.drops`. Verified against the unfixed walk: the
  engine-level `an_absurd_tempo_does_not_wedge_the_step_walk` **hangs** (killed at 90 s, the harness
  reporting ">60 s") and `a_dense_grid_does_not_run_away_in_one_block` **fails**; both pass
  on the fix.
- Tests: `graph::tests::an_absurd_tempo_does_not_wedge_the_step_walk` and
  `graph::tests::a_dense_grid_does_not_run_away_in_one_block` (node level, exact counts —
  `i64::MAX - EUCLIDEAN_STEP_CAP` and `709_724 - EUCLIDEAN_STEP_CAP`); and in
  `crates/engine/tests/euclidean_tempo.rs`, the user-facing path:
  `an_absurd_tempo_does_not_wedge_the_step_walk`,
  `a_dense_grid_does_not_run_away_in_one_block`,
  `a_sane_tempo_walks_its_whole_grid_and_counts_nothing` (a 120 and 240 bpm session counts nothing,
  so the bound is provably outside the audible range) and `unmounting_withdraws_the_drop_counter`.
- **The audible output is unchanged for every tempo the suite covers**, and unchanged in fact up to
  ~1.2e5 bpm at the default subdivision. The cap truncates only where the grid is finer than the
  sample grid, which is exactly where `frame_at` could not have placed the steps distinctly anyway.
- **The counter's meaning, restated because an operator has to interpret it:** grid steps a block
  did not evaluate. Not "onsets that would have sounded" — a false pattern position emits nothing by
  design — and not a count of triggers. A nonzero value is a loud bound on an absurd tempo, never a
  silent truncation; the same number is the difference between what the block's grid held and what
  the block looked at.
- **The render path still allocates nothing.** The `Arc<AtomicU64>` is built on the control side at
  apply, and the render path only `fetch_add`s. The counting-allocator tests
  (`render_path_does_not_allocate`, `render_path_does_not_allocate_with_latency`) and the rest of the
  engine suite (102 tests) pass unchanged.
- **A source-breaking change, stated plainly:** `EuclideanGen { steps, pulses_per_beat, pattern }` no
  longer compiles outside the crate (the counter field is private) — use `EuclideanGen::new` or
  `with_drop_counter`. `crates/engine/src/plugins/euclidean.rs` is the only construction site in the
  tree, so nothing else in the workspace moved, and the constructor's length assertion is now
  impossible to route around.
- `EUCLIDEAN_STEP_CAP` and `EUCLIDEAN_DROPS_KEY` are re-exported from the crate root, so a shell can
  name the bound and the key the way `MIN_TEMPO_BPM` is named today.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
