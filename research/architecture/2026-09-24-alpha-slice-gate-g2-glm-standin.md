# Reviewer gate (stand-in) — alpha slice G2 (seek at scale), GLM-5.3

> Research input, 2026-09-24. Co-worker: **GLM-5.3** (`zai/glm-5.3`), read-only, standing in for the
> designated gate **Kimi K3** (unusable: every API run died with no output — see
> [`2026-09-23-kimi-gate-alpha-slices.md`](2026-09-23-kimi-gate-alpha-slices.md)).
> Scope: item 15 — the **warm-up seek** (`Clock::seek_to`, `can_warm_seek`, `rebuild_prefix`, the
> reported `last_seek`), the arranger's EOF clamp the slice needed, and the tempo handling the run-in
> required. The reviewer compiled out-of-repo probes against the built rlibs and drove release
> `host v1` scripts (`/tmp/probe/loop.rs`, `/tmp/probe/clamp.rs`, a 30-minute four-track fixture).
>
> **Disposition: `merge with changes`; both must-fix findings (one statement) and the should-fix were
> real and are fixed, with tests.** Its must-fixes were both in the clamp *this slice added* — an
> optimisation that would have regressed a working seek, caught because the reviewer executed the
> cases the slice's own tests did not cover.

## Must-fix

1. **The EOF clamp broke a looped clip's phase.** `ArrangerNode::new` bounds the mount phase by the
   source's length so an over-declared *contiguous* clip mounts instead of failing — but a looped
   clip's phase is `off0 % loop_len` and its cycle `off0 / loop_len`, so the clamp moved both. The
   reviewer's repro (60 000-frame file, `loop_len: Some(48 000)`, `src_len: 192 000`, mount at
   97 000): debug panicked on the slice's own alignment invariant
   (`clip 'c0' reader slipped (off 97000, popped 0 + off0 60000)`), release played the **wrong cycle**
   (`equal: false` against the full path). **Fixed**: the EOF bound applies only to a contiguous read
   (`loop_len: None`); a looping reader past EOF is already dead by construction. Tested in
   `the_eof_clamp_respects_src_start_and_loops`.
2. **The clamp ignored `src_start`.** The reader mounts at `src_start + phase`, so the material left
   is `src_frames - src_start`, not `src_frames`. The reviewer's repro (`src_start: 24 000` on a
   48 000-frame file, `src_len: 96 000`, mount at 60 000) still failed with
   `seek_frames(72000) past end (48000)` — a seek regression in exactly the class the clamp was added
   to close. **Fixed** in the same statement, and re-verified at the CLI: both repro shapes now report
   `warmed` and bounce.

## Should-fix

3. **`last_seek` was set by undo/redo too.** Both call `replay_to`, so after an undo the CLI/outcome
   announced "seek to frame X — warmed" for something that was not a seek. **Fixed**: `replay_to_kind`
   carries an `is_seek` flag (undo/redo pass false); an edit now *carries the last real seek's report
   across* rather than blanking it, pinned by `last_seek_ignores_undo_and_redo`.

## Notes (recorded, one fixed)

- **The CLI hardcoded 48 000** when printing the run-in's seconds (wrong for a 44.1 kHz session).
  Fixed to use the session's rate.
- **A backdated `set_tempo @N` (N < now)** is applied at the current clock when issued live but at N on
  a replay — a pre-existing live/replay divergence, identical on both seek paths, not this slice's to
  fix (recorded).
- **A flaky pre-existing capture test** (`a_take_records_into_the_pool_and_plays`, "capture stopped
  with a partial frame") failed once for the reviewer and passes in isolation; seen in earlier gates
  too. Its own look is owed.
- Found by **self-review while the gate ran**, and fixed before it reported: a mid-piece `SetTempo` is
  frame-placed *value* state, so `rebuild_prefix`'s `at_now` would have moved it to frame 0 — the audio
  would have been right (the render reads frames) but every beat reading, the ruler and the position
  readout would have disagreed with a full replay. Fixed by placing the clock at the command's frame
  and scheduling it there; that exposed a **latent engine bug** — a tempo event delivered *late* was
  stamped at the clock's current frame instead of its own, so `SchedEvent::SetTempo` now carries
  `at_frame` and `apply_event` pushes the segment at it. Pinned by
  `a_warm_seek_keeps_mid_piece_tempo_placement` (beat equality to the bit).

## Checked and correct (executed, not assumed)

- **The gain is real**: on the reviewer's own 30-minute, four-track, master-chain fixture, 5.0 s warm
  vs 11.9 s with a same-value `set_param` placed inside the run-in (forcing the full path) — the delta
  is the 30-minute render that is not done. `last_seek` reports `warmed`/`full` exactly as claimed.
- **Byte equality holds at an *unaligned* target** (86 000 000, not a multiple of the run-in): the two
  bounces are identical. Chunk boundaries do not affect bytes because `render_block` splits at absolute
  event frames.
- **Stale scheduler events** are delivered at the run-in's first block (the peek filter has no lower
  bound) and stamped at their own frame, and scheduler insertion is frame-sorted, so out-of-order
  history still pushes ascending tempo segments.
- **`at_frame: None` commands** are never "late"; applying them at once preserves the last-writer value
  a clock-walking replay has in force, and 1 200 frameless ops replayed byte-identically.
- **`Group`** entries are all-`Arrange` with one shared frame (validated), and `can_warm_seek` checks
  every member rather than the first.
- **Undo/redo, `record` in progress, `Play`/`Splice`** are handled (the last two excluded by the
  guard); `replay_full` is the fallback branch's statements verbatim.
- **Engine monotonicity**: `Engine::seek` leaves scheduler entries intact and they apply (not skipped,
  not doubled) in the first run-in block; PDC and the drain are untouched, and the byte-identical
  bounces go through the compensated `render_with_drain_aligned` path.
