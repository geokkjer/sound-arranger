# kimi review — P1.3 clip-editor design note (2026-08-24)

Session: `session_3846638b-c762-4272-aa63-bcc06aae29ef`

kimi reviewed the P1.3 *shape* (proposed) note before any code — a design-stage critique.
Verdict on the central decision (clip timeline as a graph **value**, one opaque
`ArrangerNode` per track): **sound, and arguably the only choice consistent with the
philosophy.** The problems are all in the details the note waved past. Its closing
diagnosis, which is accurate:

> The note's architecture is right; its failure mode is that the byte-identity
> promise is asserted in places (undo, warm-race, pool recovery, id generation)
> where it's actually conditional, and each condition needs to be made explicit
> and logged.

## Must-fix (all integrated into the note)

1. **Undo as stated breaks the core rule.** If the UI swaps in an old snapshot without
   logging it, visible model and log diverge and byte-identical replay dies. Undo must
   be *logged* inverse ops (or a logged checkpoint-restore op).
2. **Boundary-fade contradiction.** The node both auto-splices at clip boundaries
   *and* applies per-clip `fade_in`/`fade_out`. Fix: per-clip fades are authoritative;
   the auto equal-power crossfade applies only when both abutting boundary fades are
   zero (or is dropped in favour of explicit fades).
3. **"One reader per active source" is wrong.** The reader unit is per **read position**
   (per active clip instance), not per source — two clips may read the same PoolId at
   different `src_start` offsets.
4. **Edit-before-warm race.** A clip placed at/near the playhead can't be warmed.
   Deterministic resolution: an op whose `at_frame` is before the current render position
   is refused (fail-loud, never logged); a clip that reaches the playhead before its
   reader is warmed yields **silence** (underruns counted, not audible) — so the *audio*
   stays a pure function of (log, pool, frame), and replay is byte-identical.
5. **Clip `id` generation must be deterministic** — a pure function of the op stream
   (sequential counter / hash of op index), never random/UUID/time. Same for the two
   halves of `RazorSplit` and the clone from `Duplicate`.
6. **Track lifecycle missing.** Add `AddTrack`/`RemoveTrack`; `MoveClip` gains a
   destination track. Track creation has **one** logged source of truth: the
   `ArrangerNode` graph mount is *driven by* the timeline value (which owns tracks), so
   they cannot desync.
7. **Pool recovery invalidates replay.** A recovered take's content (truncated tail)
   changes under the content-hash id. State the equivalence class explicitly: replay is
   over **(log, exact pool content-set)**; `Pool::recover()` is a pool-mutating event
   that forks the session and must be logged/acknowledged, not a silent repair.

## Should-fix (integrated)

8. **Canonical payload encoding.** `f32` values in a logged op use bit-exact
   serialization (`to_bits`); the log's byte-identity needs no float-formatting ambiguity.
9. **Single-envelope encoding wins, against the note's lean.** `Event::Arrangement { op,
   at_frame }` + plugin-registered dispatch: `RazorSplit` is *plugin* semantics, not a
   core concept. The std-only core must not grow one variant per plugin op. Fail-loud
   unknown-op replaces exhaustive `match`.
10. **`LoopRegion` decides: bake.** N repeats ⇒ one clip with deterministic
    `src_len = N × region`. Seam semantics stated: fades apply to the *outer* clip;
    inner seams are hard (click risk documented) unless a loop-crossfade is added later.
11. **`SetClipGain` op added** (gain is in the model, so it needs a logged op). Track
    gain is the mixer channel, stated.
12. **Active-clip budget** for the no-alloc invariant: a per-track max active clips;
    overflow refuses the op, fail-loud.
13. **`Trim` specified**: which edge, direction, and whether it moves `at_frame`.

## Worth-considering (left as flags/alternatives in the note)

- Tiny default de-click fade (~1–3 ms) on split-created boundaries, logged explicitly.
- `PoolId` interning is leaky; a fixed-size hash or explicit string in the op payload is
  cleaner and self-describing.
- A periodic logged `Timeline` checkpoint as a *complement* to the op log (not a
  per-edit snapshot) for long-session open cost; the rejected case was only per-edit
  snapshots.
- The PDC risk item is mostly a non-risk (disk sources have no plugin latency); the
  energy belongs on the warm-race (item 4).

The full critique text is the reviewer's response in this session.
