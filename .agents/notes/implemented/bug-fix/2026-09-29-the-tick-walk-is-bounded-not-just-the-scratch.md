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

## Decision

**Three independent bounds, so no single one carries the invariant.**

- **`TempoMap::frame_at` saturates.** An unreachable beat answers `u64::MAX` — "past the end of time" —
  instead of a real frame. Every frame-walking caller now terminates on it. This also removes an
  `expect("tempo map never empty")` from a path the render thread reaches.
- **The emit loop is bounded by `CLOCK_OUT_CAP` ticks, and the rest is counted exactly.** The cap moves
  from the scratch to the walk: after the cap the node stops walking and `fetch_add`s the block's
  remaining tick count. The count is exact, so `overflows` keeps meaning "exactly how many ticks this
  block dropped" — the existing `overflow_is_counted_never_grown` still asserts `emitted + overflows ==
  due` at 1e9 bpm. `frame_at` is monotone in the tick index, so `due_ticks_from` finds that count by
  doubling-then-binary search: a fixed ~128 `frame_at` lookups (each O(segments), allocation-free)
  instead of the walk it replaces. The tick offset is now `saturating_sub`, so a tick the bounded
  correction could not walk onto the block's frame lands at offset 0 instead of underflowing.
- **`first_tick_at_or_after`'s correction walk is bounded by `TICK_WALK_CAP` (65536).** The walk exists
  to undo f64 rounding, so it is one or two ticks at any tempo where a tick is worth a frame; a generous
  bound never changes the answer, and a walk that cannot converge must still return.
- **The host refuses a sub-floor tempo.** `Engine::set_tempo` now also rejects `bpm < MIN_TEMPO_BPM`
  (1e-3 bpm — a quarter note longer than the timeline itself) with a message naming the floor, and
  never logs it. This is the user-reachable half: the script, `session.txt` and journal paths all meet
  `set_tempo` first, so a nonsense tempo is an `Err` at the command boundary rather than a tempo map no
  tick can be placed in. `MIN_TEMPO_BPM` is re-exported from the crate root.

The three bounds are deliberately redundant. The floor alone leaves a hand-built `TempoMap` (an embedder,
a replayed log from an older version) exposed; the loop cap alone would leave `frame_at` lying to every
other caller; the saturation alone leaves the fast-tempo hang. The invariant is *the render path
terminates*, and it now holds for each of them separately.

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
- **Update `overflow_is_counted_never_grown` to the new counting path.** Not needed, and that is the
  point: the exact count keeps the old test true unchanged, so the regression is covered by an existing
  assertion rather than a rewritten one.

## Consequences

- Rendered: `frame_at` saturates on an unreachable beat, the emit loop and the correction walk are both
  bounded, the overflow counter still counts exactly, and a sub-floor `set_tempo` is a named `Err`.
- Tests: `frame_at_saturates_on_a_tempo_too_slow_to_reach_the_beat` (engine lib, beside the other
  `TempoMap` tests) and `an_absurd_tempo_does_not_hold_the_render_loop` (clock-out module, beside
  `overflow_is_counted_never_grown`) both hang or fail on the unfixed code and pass on the fix; the
  engine-level `a_tempo_below_the_floor_is_refused_and_the_session_still_renders` covers the
  user-reachable half. `overflow_is_counted_never_grown` is unchanged and still passes.
- The render path stays allocation-free: `due_ticks_from` allocates nothing and runs only on a block
  that already exceeded the cap, so the counting-allocator tests
  (`render_path_does_not_allocate`, `render_path_does_not_allocate_with_latency`) are unaffected — they
  pass.
- **The floor is a policy, and it is narrow on purpose.** 1e-3 bpm is ~16.7 minutes per beat, so nothing
  musical is excluded; a real slow tempo (a whole note over a minute) is far above it. A *ceiling* is the
  symmetric question and is **not** decided here — the fast-tempo hang is already impossible, but a
  session may still carry a tempo that floods the counter, and whether that should be refused at
  `set_tempo` or merely reported through `overflows` is a product call, not a correctness one.
- `MIN_TEMPO_BPM` is now part of the engine's public surface, so a shell can show the floor in a tempo
  field rather than letting a user discover it as an error string.

*Authored with Space Bunny · OpenCode, 2026-09-29. Reviewed and committed by DeepSeek-V4.1-Flash · DeepSeek Harness.*
