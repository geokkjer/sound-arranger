# Agent Note: The tick walk is bounded, not just the scratch

Status: implemented

## Problem

The [clock-out note](../../implemented/architecture/2026-09-29-midi-clock-out-in-the-engine.md) bounded
`CLOCK_OUT_CAP` **events per block** and grew the counter for the rest. The bound was on the *scratch*;
the **walk** that fills it had no bound, and a walk with no bound is a permanent process hang — worse
than a panic, because a panic unwinds a thread and a hang never does.

The walk runs `n` upward from the block's first due tick until a tick frame reaches the block end. Two
ways it never gets there:

- **A tempo below ~6.5e-15 bpm.** `TempoMap::frame_at` documented its tail as "Unreachable: the final
  open-ended segment covers any finite beat", and it was reachable. With one open segment
  `seg_beats = u64::MAX as f64 × bpm / 60 / sample_rate`; below ~6.5e-15 bpm at 48 kHz that is under
  `1/24` of a beat, so every beat after the first fell through to the map's last start frame — `0`.
  `frame_at` was a constant, `first_tick_at_or_after`'s correction walked a constant, and the emit loop
  read `frame >= block_end` as never. Nothing clamped on the way in: `parse_script` accepts any `f64`,
  and `set_tempo` checked only `is_finite() && bpm > 0.0`, so `set_tempo 0.000000000000001 4` in a
  script, a `session.txt` or a `journal.txt` reached the tempo map. Opening such a session and pressing
  play wedged the actor thread: `render` never returned, no further command was serviced, the shell
  froze, and `overflows` climbed forever.
- **A tempo fast enough that every tick rounds onto frame 0** (1e300 bpm and up): the same constant, the
  other way round, and the same non-returning loop.

The overflow test sat at 1e9 bpm, where ticks are ~1.2e-4 frames apart, so `frame_at` still resolved and
the loop terminated after a bounded 4.27 M steps. The test was far above the threshold and could not see
either failure.

### What the first cut of this fix left behind

An independent review of the landed fix ([commit 5acac32]) found that it closed the slow half and left the
same walk reachable from the fast side, plus three claims that did not survive checking:

- **`n += 1` overflowed `u64`.** `TICK_WALK_CAP` bounded the *distance* walked, not the *index*, and the
  cap's own guard (`n - n0 < TICK_WALK_CAP`) was itself an unchecked subtraction from a saturated `n0`.
  At a tempo whose ticks are worth ≪ 1 frame and `block.frame >= 1`, `beat_at(frame) * 24.0` saturates
  `n0` to `u64::MAX`, `tick_frame(u64::MAX)` rounds to `0 < frame`, and the walk stepped off the top —
  a debug panic on the render thread (`set_tempo(1e300, 4)`, which the floor accepts: the floor bounds the
  slow side only), and in release a wrap to 0 that restarted the walk from tick 0, i.e. the original hang
  arriving through the overflow instead of past it. The emit loop's `n += 1` had the same exposure.
- **The `MIN_TEMPO_BPM` doc comment's arithmetic was wrong in both directions.** It said a beat of 1e-3 bpm
  is "~1000 s" (it is 60 000 s — 16.7 hours) and that a quarter note is "longer than the timeline itself"
  (the `u64` frame range is ~1.2e7 years at 48 kHz; the timeline is not what runs out).
- **`due_ticks_from`'s count could be understated by one.** The comment claimed the saturated case could
  only ever *overstate*. It could also come up short: when every index is due, the doubling bracket had no
  "not due" end to converge against and the search settled one below the top. So the commit message's
  "exactly" was one tick optimistic at the top of the range.

## Decision

**Three independent bounds, so no single one carries the invariant.**

- **`TempoMap::frame_at` saturates.** An unreachable beat answers `u64::MAX` — "past the end of time" —
  instead of a real frame. Every frame-walking caller now terminates on it. This also removes an
  `expect("tempo map never empty")` from a path the render thread reaches. **The saturation is the
  whole contract now, not one arm of it:** the sum that places a beat inside a segment is
  `saturating_add` too, and the doc states that the frame domain is *total* (every `f64` beat answers,
  nothing panics) and monotone in the beat. An independent verification of this fix found the sum
  unchecked twenty lines under the saturation — a debug panic on the render thread, and in release a
  frame near zero for a beat past the end of time — so the invariant this note claims held for the
  render path did not, in exactly the function it names. That correction is the
  [frame-at totality note](./2026-09-29-frame-at-is-total-the-sum-saturates-too.md), and it is what
  the "no step overflows" half of this decision rests on.
- **The emit loop is bounded by `CLOCK_OUT_CAP` ticks, and the rest is counted exactly.** The cap moves
  from the scratch to the walk: after the cap the node stops walking and `fetch_add`s the block's
  remaining tick count. The count is exact, so `overflows` keeps meaning "exactly how many ticks this
  block dropped" — the existing `overflow_is_counted_never_grown` still asserts `emitted + overflows ==
  due` at 1e9 bpm. `frame_at` is monotone in the tick index, so `due_ticks_from` finds that count by
  doubling-then-binary search: a fixed ~128 `frame_at` lookups (each O(segments), allocation-free)
  instead of the walk it replaces. The tick offset is now `saturating_sub`, so a tick the bounded
  correction could not walk onto the block's frame lands at offset 0 instead of underflowing.
- **`first_tick_at_or_after`'s correction walk is bounded by `TICK_WALK_CAP` (65536) in *both*
  directions.** The walk exists to undo f64 rounding, so it is one or two ticks at any tempo where a tick
  is worth a frame; a generous bound never changes the answer, and a walk that cannot converge must still
  return. The downward walk was previously uncapped and it does have a trigger: a map whose `frame_at`
  disagrees with its `beat_at` by more than rounding — a fast segment followed by a sub-floor one is
  enough, because `frame_at` then resolves the whole neighbourhood of the candidate onto one frame and
  the walk steps ~1e14 times before that frame rounds down. A hand-built or replayed `TempoMap` can hold
  such a pair, so the bound applies down as well as up.
- **Every step of the walk is checked, and "no such tick" is an answer.** `frame_at` is monotone in the
  tick index, so the *last* index answers for all of them: if even `tick_frame(u64::MAX)` lands short of
  the block's first frame, no tick exists at or after it and `first_tick_at_or_after` returns `None`.
  That is the honest reading — the block carries no ticks at all — and it is what stops the walk at the
  top of the range without an increment that can overflow. `render` then sends nothing and counts
  nothing: a block with no due tick has dropped no ticks, so `overflows` stays where it is rather than
  counting ticks that never existed. The increments that remain are `checked_add` against a non-monotone
  map, and the emit loop's increment is checked for the same reason: `u64::MAX` is the last tick index
  there is, and every due one has been emitted by then.
- **`due_ticks_from` is exact at the top of the range as well as the bottom.** The saturated case — every
  index due, so there is no "not due" tick to bracket the search against — is answered before the bracket
  is built: the count is every index from `from` on. That makes the commit message's "exactly" true, and
  the one case where it cannot be true (a count that does not fit in `u64` at all, from `from == 0`) is
  the only saturation left in the count.
- **The host refuses a sub-floor tempo.** `Engine::set_tempo` now also rejects `bpm < MIN_TEMPO_BPM`
  (1e-3 bpm — a quarter note lasting 16.7 hours, and some 1.5e11× above the ~6.5e-15 bpm point where the
  tempo map can no longer place a tick at all) with a message naming the floor, and never logs it. This
  is the user-reachable half for the slow end: the script, `session.txt` and journal paths all meet
  `set_tempo` first, so a nonsense tempo is an `Err` at the command boundary rather than a tempo map no
  tick can be placed in. `MIN_TEMPO_BPM` is re-exported from the crate root.

The bounds are deliberately redundant. The floor alone leaves a hand-built `TempoMap` (an embedder, a
replayed log from an older version) exposed and does nothing for the fast end at all; the loop cap alone
would leave `frame_at` lying to every other caller; the saturation alone leaves the fast-tempo hang. The
invariant is *the render path terminates without panicking*, and it now holds for each of them
separately.

## Alternatives considered

- **Only the tempo floor.** Rejected: it is the guard at the door, not the invariant. `TempoMap::push`
  is public, `Clock::push_tempo` is public, and the apply path pushes straight from the log
  (`render.rs`: `self.clock.tempo_map.push(at_frame, bpm, beats_per_bar)`) without re-validating — a log
  written before the floor existed would still reach the map, and a session file is user-writable.
- **Only the loop cap, no `frame_at` change.** Rejected: it stops the hang but leaves `frame_at`
  returning `0` for a beat the map cannot represent, so `frame_at(beat) < frame` stays true for every
  `beat` and the function is a lie every other caller can read. The TUI's ruler computes
  `tempo.frame_at(tempo.beat_at(view.start).ceil() + 1.0) - tempo.frame_at(...)` as a pixels-per-beat
  divisor; a zero there is a division by zero in a shell.
- **Bound the walk by a wall-clock deadline or a tick budget checked against the block length.** Rejected:
  a time-based bound makes rendering non-deterministic, which breaks the core `same log ⇒ byte-identical
  bounce` invariant outright. A tick budget is what the cap already is.
- **Emit ticks lazily / stream them to the sink instead of buffering.** Rejected: it allocates on the
  render path or sends an unbounded number of events per block, and the cap exists precisely because
  "what fits" must be well defined.
- **Refuse a sub-floor tempo in `parse_script` too.** Deferred: the host's `set_tempo` is the one
  validation point every script, session file and journal path shares, and duplicating the check in the
  parser would give two places to drift. A parse-time check would only turn the same refusal into an
  earlier one.
- **Let the slow-tempo case emit nothing rather than tick 0.** Rejected: tick 0 *is* at frame 0 at any
  tempo, and the walk finds it exactly. Dropping it would be a silent drop for no gain.
- **Clamp the tick index instead of returning `None` — e.g. pin `n` at `u64::MAX` and let the emit loop
  discover there is nothing there.** Rejected: it pushes the sentinel into the caller, which then reads
  `tick_frame(u64::MAX) == 0` and emits a block's worth of bogus ticks at offset 0 (what the bounded walk
  lands on is *not* the block's first frame, and the loop has no way to tell). `None` states the fact at
  the place that knows it, and the emit loop's "not due" test already meant what it needed to mean.
- **Bound the overflow *count* on the fast side the way `overflows` bounds the emit loop** — refuse a
  tempo above some ceiling. Deferred, as below: the walk is already bounded, so this is a product call
  about how much clock flooding to tolerate, not a correctness fix.
- **Make the overflow count saturate at a "many" value rather than the true range.** Rejected: the count
  is the only thing telling an operator how far the block was truncated, and `u64::MAX - 63` is the honest
  answer for "every remaining index".
- **Update `overflow_is_counted_never_grown` to the new counting path.** Not needed, and that is the
  point: the exact count keeps the old test true unchanged, so the regression is covered by an existing
  assertion rather than a rewritten one.

## Consequences

- Rendered: `frame_at` saturates on an unreachable beat, the emit loop and both directions of the
  correction walk are bounded, no step of either overflows an index, `first_tick_at_or_after` reports
  "no such tick" as `None`, the overflow counter counts exactly including at the top of the range, and a
  sub-floor `set_tempo` is a named `Err`.
- Tests: `frame_at_saturates_on_a_tempo_too_slow_to_reach_the_beat` (engine lib, beside the other
  `TempoMap` tests) and `an_absurd_tempo_does_not_hold_the_render_loop` (clock-out module, beside
  `overflow_is_counted_never_grown`) both hang or fail on the unfixed code and pass on the fix; the
  engine-level `a_tempo_below_the_floor_is_refused_and_the_session_still_renders` covers the
  user-reachable slow end. `overflow_is_counted_never_grown` is unchanged and still passes. The
  follow-up adds `a_fast_tempo_one_block_in_does_not_panic_or_walk` (the fast side across a block
  boundary — the one the first cut missed, because `an_absurd_tempo_does_not_hold_the_render_loop` only
  renders `block.frame == 0`, which the sentinel never reaches) and `the_downward_walk_is_bounded`
  (the uncapped downward walk, with a trigger map that never returns uncapped), both clock-out module
  tests; and `an_absurd_fast_tempo_renders_the_next_block` in `tests/clock_out.rs`, which reaches the
  same defect through the API a user touches — `set_tempo(1e300, 4)` — and renders four blocks.
- **The claim that no step overflows needed one more fix to be true, and it was not only about the
  walk.** `TempoMap::frame_at`'s own `start_frame + frames` was unchecked, so the invariant this note
  states — *the render path terminates without panicking* — was false in the very function it names as
  one of the three bounds. The [frame-at totality note](./2026-09-29-frame-at-is-total-the-sum-saturates-too.md)
  saturates that sum (and `Clock::advance`'s), adds
  `frame_at_saturates_the_sum_when_a_seek_puts_a_tempo_near_the_top`,
  `advance_saturates_at_the_top_of_the_frame_range` and `frame_at_is_total_over_the_beat_domain`
  beside the other `TempoMap` tests, and records the two render-path `block.frame + len` sums in
  `ClockOutNode::render` and `EuclideanGen::render` that it found and deliberately did not fix — so a
  seek near the top of the range is survivable in the tempo map and in the clock, but **not yet end to
  end**: "no panic after a seek near `u64::MAX`" is a stated open item, not a shipped property.
- The render path stays allocation-free: `due_ticks_from` allocates nothing and runs only on a block
  that already exceeded the cap, so the counting-allocator tests
  (`render_path_does_not_allocate`, `render_path_does_not_allocate_with_latency`) are unaffected — they
  pass.
- **The floor is a policy, and it is narrow on purpose.** 1e-3 bpm is 16.7 hours per beat, so nothing
  musical is excluded; a real slow tempo (a whole note over a minute) is some 50× above it. The floor is
  justified by what the tempo map can express, not by the timeline running out: below ~6.5e-15 bpm at
  48 kHz the final open-ended segment spans less than 1/24 of a beat and no tick can be placed, while the
  whole `u64` frame range at 48 kHz is ~1.2e7 years. A *ceiling* is the symmetric question and is **not**
  decided here — the fast-tempo hang and panic are both impossible now, but a session may still carry a
  tempo that floods the counter, and whether that should be refused at `set_tempo` or merely reported
  through `overflows` is a product call, not a correctness one.
- `MIN_TEMPO_BPM` is now part of the engine's public surface, so a shell can show the floor in a tempo
  field rather than letting a user discover it as an error string.
- **The euclidean node's step walk — the second tempo-derived walk this note's commit left open — is
  bounded in the [step-walk note](./2026-09-29-the-euclidean-step-walk-is-bounded-and-counted.md).**
  `EuclideanGen` reads `beat_at`/`frame_at` on the render path too, its loop length came from the
  same `beat_at`, and at 1e300 bpm it walked 9.2e18 steps on the first block. Those two nodes are the
  only render-path readers of the tempo map, so "no tempo can hang the render" holds once both
  walks are capped.

*Authored with Space Bunny · OpenCode, 2026-09-29. Reviewed and committed by DeepSeek-V4.1-Flash · DeepSeek Harness. Corrected by Space Bunny · OpenCode, 2026-09-29 after an independent review of the landed fix.*
