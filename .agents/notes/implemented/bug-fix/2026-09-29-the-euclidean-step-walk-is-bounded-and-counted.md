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
bounded in the next commit". This is that commit, and the review of it.

**Bounding the walk was necessary and not sufficient**, which is the shape this node keeps teaching:
the walk is one of three quantities a block spends, and the cap only reached one of them. The review
of the capped version confirmed the bound and found the other two, plus the per-step cost the cap
multiplies:

- **The invariant the constructor asserts was not the one the fields could not break.** Only the
  counter field was private, so `let mut n = EuclideanGen::new(8, 4, vec![false; 8]); n.steps = 64;`
  reintroduced the out-of-bounds pattern index on the render thread — a panic, from safe code, in the
  very node whose bound was the point.
- **The pattern's length was never bounded at all**, which is a bigger hole than the walk: it is one
  `bool` per step, held three times over, and it comes from a mount param that is checked for
  finiteness and nothing else. `steps: get("steps", 8.0) as u32` saturates, so
  `mount euclidean steps=4294967295` allocated ~4 GB on the render thread, three times, from a
  30-byte script line. Bounding the *walk* never touched it.
- **The per-step cost the new cap multiplies**: `TempoMap::frame_at` scans the tempo map from its
  first segment, so a capped block could perform up to 4096 full scans. Measured and bounded below
  rather than redesigned here.

## Decision

**The block's grid is counted before it is walked, the walk is capped, and the remainder is
counted and published** — the clock-out node's three-part shape (`frame_at` saturates · the walk is
capped · the overflow is counted) applied to the node the first cut left behind — **and then the
node's own shape is closed around it: the fields are private, so the length invariant cannot be
broken after construction, and the pattern's length is bounded by one named constant at every point
that allocates it, starting with the factory that reads the mount param.**

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
- **The node's fields are private and the constructor is the only way in** — `steps`,
  `pulses_per_beat` and `pattern` are read through `EuclideanGen::steps()`,
  `pulses_per_beat()` and `pattern()`. The counter field was already private, but that left the
  length invariant *asserted* and not *enforced*: the assert fires at construction and nothing held
  it afterwards, so `let mut n = EuclideanGen::new(8, 4, vec![false; 8]); n.steps = 64;` put the
  out-of-bounds index straight back — a panic on the render thread, from a safe public API. The
  render path also indexes by `pattern.len()` rather than by `steps.max(1)`, so the index is in
  bounds by construction even if some later edit moved the assertion. The verifier's reproduction
  is a `compile_fail` doctest on the struct, so it cannot come back unnoticed.
- **The pattern's length is bounded by `EUCLIDEAN_MAX_STEPS` = 4096, and the mount param is
  refused at the door.** This was the pre-existing half of the same defect: the cap bounded the
  *walk*, never the *pattern*, and a mount param is checked for finiteness and nothing else, so
  `steps: get("steps", 8.0) as u32` **saturates** and `mount euclidean steps=4294967295` reached
  `vec![false; n]` — ~4 GB, cloned once more into the node and retained a third time in the
  `Rhythm` service, all of it on the render thread, from a 30-byte script line. One named constant
  in `graph.rs`, refused in three places: `euclidean_factory` returns the error (so
  `Engine::mount`, which dry-runs the factory, refuses **synchronously and loudly**, naming the
  value and the limit), `Euclidean::apply` returns the same error (its own fields are public, and
  `apply` runs on the render thread, which must not panic), and `EuclideanGen`'s constructor and
  `euclid` assert the bound before allocating. 4096 steps is far past any rhythm a person writes —
  a bar of 64th notes in 4/4 is 64 — and it keeps three copies of the pattern a page of memory.

### The per-step cost the cap multiplies — a known cost, not a fix

`block.tempo.frame_at(index as f64 * step_beats)` is a **linear scan of the tempo map from its
first segment**, so it is O(segments) and not O(1); the walk cap multiplies it. The loose bound is
`EUCLIDEAN_STEP_CAP × segments` (4096 × one segment per `set_tempo` in the log) iterations per
block, and the verifier's "up to 4096 full scans" is that loose bound. It was instrumented and
measured on a 512-frame block at 1e9 bpm with an all-true 8-step pattern:

| map | `frame_at` calls | segment visits |
| --- | --- | --- |
| 1 segment, 1e9 bpm, all-true | 32 | 32 |
| 64 segments, block deep in the map, all-true | 32 | 2 048 |
| 4 096 segments, block deep in the map, all-true | 32 | 32 031 |
| 1 segment, 1e9 bpm, one pulse in 1024 steps | 4 | 4 |
| 64 segments, 120 bpm, all-true | 2 | 8 |

The call count is **32 whatever the tempo**, because only pattern-*true* steps reach `frame_at` and
each in-block one pushes, so the trigger buffer's 32 entries end the walk before the 4096th step.
The remaining question is whether a map can make `frame_at` answer out-of-block forever, so the
buffer never fills and the walk does run to the cap: that needs a segment that makes `frame_at`
saturate at `u64::MAX`, which needs a non-positive bpm, and a negative bpm also makes `beat_at`
*decrease* across the block, so `s1 ≤ s0` and `grid` is 0 — the walk never starts. So the measured
32 is not a happy accident of the pattern; it is what a monotone map forces, and the one map shape
that could beat it collapses the walk instead. **Left unfixed on purpose:** making `frame_at`
O(log n) means prefix sums on the tempo map, maintained in `TempoMap::push` — a `clock` change
that also moves `beat_at` and the clock-out node's tick walk, not a node-local fix. At 32 scans of
a map a session builds one segment per tempo change, the cost is real and small, and the note that
should decide it is a `clock` note.

### Is "no tempo can hang the render" true now?

Yes, and here is exactly what that rests on: the tempo map is read on the render path in **two**
places — `ClockOutNode::render` and `EuclideanGen::render`; `rg 'beat_at|frame_at'` over `crates/`
finds those two plus the `TempoMap` implementation itself and doc prose. Both walks are now capped
and count what they skip, so no tempo makes any node's render loop unbounded.

Nor can a session make this node allocate: the pattern is bounded by `EUCLIDEAN_MAX_STEPS` before
the first allocation, and the fields that hold its length invariant cannot be reached except through
the constructor that checks them, so the render thread's worst case is a 4096-step walk over a
4096-long pattern.

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
  only way to build a node — which is the only reason the length assertion can mean anything. The
  verifier then showed the half-measure is not enough: making only the *counter* private left
  `steps` public, and a public field cannot hold an invariant that a constructor asserted. All four
  fields are private now, and the reproduction is a `compile_fail` doctest.
- **A per-parameter range table in the engine, so `mount` refuses out-of-range params generically.**
  Rejected: it is a new mechanism (a declared range per parameter, a home for it, a message shape)
  for one parameter, and it puts each plugin's domain in the engine instead of the plugin that owns
  it. The factory *is* where a mount param is read, and `Engine::validate_mount` already dry-runs it
  and surfaces its `Err` synchronously — so the plugin's own bound is already a door, not a
  convention, and needs no new plumbing. Worth revisiting when a *second* plugin wants a bounded
  numeric param, since that is when the shared shape starts paying for itself.
- **Make the constructor return a `Result` instead of asserting.** Rejected: a `steps` over the
  limit is a caller bug, not a runtime condition, and the callers are the plugin (which refuses
  before it gets there) and a direct `new` in test or shell code. A `Result` would push the same
  `?` onto every construction site to handle a mistake the type system could not have let through,
  and `new` is not on the render path — `apply` is, and it returns an error there.
- **Make `frame_at` O(log n) here, with prefix sums maintained in `TempoMap::push`.** Rejected *as
  part of this note*, not on the merits: it is the right fix, and it is a `clock` change, because
  `beat_at` reads the same sums and the clock-out node's tick walk is the other caller. The cheap
  local version — anchoring the scan at the block's own segment — was rejected on correctness: a
  block's step range can straddle a tempo change, and on the non-monotone map this codebase still
  tolerates there is no "the block's own segment" to anchor to. A wrong trigger offset is audible;
  a slower-but-identical scan is not. Measured cost while it stands: 32 scans per block, table
  above.
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
- **The two follow-up defects have their own tests**, from the same review:
  `a_node_is_built_through_its_constructor_and_read_through_accessors`,
  `a_pattern_shorter_than_its_steps_is_refused` and
  `an_absurd_step_count_is_refused_before_it_is_allocated` (node level);
  `the_factory_refuses_an_absurd_step_count`, `the_factory_still_builds_at_the_limit` and
  `euclid_refuses_an_absurd_step_count` (plugin level); and at the API a user touches,
  `euclidean_tempo::an_absurd_step_count_is_refused_and_the_session_still_renders`, which mounts
  `steps=4294967295`, asserts the refusal names both the value and the limit, then shows the refusal
  was *complete* — no instance, no trigger provider, the engine still renders a block, and the next
  `mount euclidean` of the same name succeeds. The verifier's field-mutation reproduction is a
  `compile_fail` doctest on `EuclideanGen`, so a public `steps` fails the suite rather than
  reverting quietly.
- **The audible output is unchanged for every tempo the suite covers**, and unchanged in fact up to
  ~1.2e5 bpm at the default subdivision. The cap truncates only where the grid is finer than the
  sample grid, which is exactly where `frame_at` could not have placed the steps distinctly anyway.
  The pattern bound changes nothing audible either: every `steps` in the tree is 8, except the
  sparse 1024-step fixture in `tests/euclidean_tempo.rs` — a quarter of the bound, and the fixture
  exists to make the *walk* cap the thing that stops a block.
- **The counter's meaning, restated because an operator has to interpret it:** grid steps a block
  did not evaluate. Not "onsets that would have sounded" — a false pattern position emits nothing by
  design — and not a count of triggers. A nonzero value is a loud bound on an absurd tempo, never a
  silent truncation; the same number is the difference between what the block's grid held and what
  the block looked at. The pattern bound counts nothing at all: it refuses the mount, it does not
  clamp a `steps` into range.
- **The render path still allocates nothing.** The `Arc<AtomicU64>` is built on the control side at
  apply, and the render path only `fetch_add`s. The counting-allocator tests
  (`render_path_does_not_allocate`, `render_path_does_not_allocate_with_latency`) and the rest of the
  engine suite (114 tests, two of them doctests) pass unchanged.
- **A source-breaking change, stated plainly:** `EuclideanGen { steps, pulses_per_beat, pattern }` no
  longer compiles outside the crate — every field is private now, not just the counter — so build one
  with `EuclideanGen::new` / `with_drop_counter` and read it with `steps()`,
  `pulses_per_beat()`, `pattern()`. `crates/engine/src/plugins/euclidean.rs` is the only
  construction site in the tree, so nothing else in the workspace moved (`cargo check --workspace`
  is clean), and the constructor's two assertions are now impossible to route around.
- `EUCLIDEAN_STEP_CAP`, `EUCLIDEAN_MAX_STEPS` and `EUCLIDEAN_DROPS_KEY` are re-exported from the
  crate root, so a shell can name the bounds and the key the way `MIN_TEMPO_BPM` is named today.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
