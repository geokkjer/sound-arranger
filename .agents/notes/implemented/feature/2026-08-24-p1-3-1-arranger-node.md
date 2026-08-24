# Agent Note: P1.3.1 — the ArrangerNode (one opaque node per track)

Status: implemented

## Problem

P1.3.0 shipped the `Timeline` value and ACID ops. The arrangement still needs to *sound*:
a node that reads a track's clips and streams the pool sources to audio on the render path.
Before this slice there was no way to render a track — the host's old `Play`/`Splice` path
was a single player, not an arrangement interpreter.

## Decision

`crates/media/src/arranger.rs` adds **`ArrangerNode`**, one opaque `engine::AudioNode` per
track (`out("audio")`), interpreting a [`Timeline`] value's clips on the render path:

- **Active-clip selection** by scanning clips (sorted by `at_frame`) for overlap with
  `[frame, frame+BLOCK)`; a fully-past clip is skipped, a fully-future clip breaks.
- **Per-read-position streaming** — one `FilePlayer` reader ring per active clip *instance*
  (a source read at two `src_start` offsets is two readers), warmed on the control side and
  detached (never joined) on retire.
- **Overlap summing** — layered clips sum (gaps are silence).
- **Per-clip fades** — `fade_in`/`fade_out` are *authoritative*: linear ramps, `min(gin, gout)`;
  there is no hidden auto-splice/equal-power crossfade, so no double-fade at boundaries.
- **Baked loops** — `FilePlayer::start_looped` wraps the read back to `clip.start` every
  `loop_len` frames (the value's `loop_len`), producing `src_len` frames by repetition.

`FilePlayer` gained `start_looped` (`crates/media/src/stream.rs`); `start` now delegates
with `loop_len = None`.

**Determinism scope (the kimi slice-2 must-fix):** byte-identical output requires a reader
that never underruns. An underrun is counted and is a *hard error the bounce must surface*
(assert `underruns == 0`); it cannot be recovered without shifting every later sample of
that clip (an SPSC ring has no random access). The node asserts `popped == off` (guarded on
"reader still has data") in debug so a test that slips fails loudly instead of shipping
shifted audio. The profile must render **contiguously from frame 0**.

## Alternatives considered

- **A mailbox/splice node (reuse `PlaybackNode`+`SpliceCmd`)** — rejected; the arranger's
  boundaries are *clips placed at absolute frames*, not a running player being spliced.
  Per-clip fades are the boundary treatment; the equal-power splice is a different concern.
- **One `ArrangerNode` for the whole canvas (N tracks in one node)** — rejected for now;
  one node per track keeps each track independently routable and the fade/sum logic per
  track. A single-canvas node is a later consolidation.
- **Load whole clips into memory instead of streaming** — rejected; the profile's sources are
  recorded jams that can be long, and disk streaming is the whole point (Spike B).
- **Block the render path on a not-yet-warmed reader** — rejected (violates no-alloc/no-block).
  The honest resolution is to scope the guarantee to no-underrun and enforce it at the bounce.

## Consequences

- A track renders: contiguous clips at their absolute frames, per-clip fades applied,
  overlaps summed, gaps silent, loops wrapped — all without allocating on the render path.
- The `underruns` counter is observable (an underrun is a hard error, not a silent shift);
  `popped == off` is the debug alignment guard.
- `FilePlayer::start_looped`'s loop math was verified by kimi (no over-read/double-wrap; a
  short source terminates deterministically rather than seeking forever).
- 6 arranger tests pass (exact-content, exact-alignment at a nonzero `at_frame`, overlap sum,
  fade ramps, loop wrap with an observable ramp, EOF-as-silence), plus the `start_looped`
  path. Workspace: 40 media unit tests, clippy clean.
- **Deferred**: live value updates (the node currently snapshots a `Track` at construction;
  P1.3.2 wires engine dispatch + reader reconciliation and the log-visibility carve-out);
  pooling the readers (P1.3.3); `PoolResolver` is the id→path seam the pool will supply.
