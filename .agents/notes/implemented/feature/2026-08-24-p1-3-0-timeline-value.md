# Agent Note: P1.3.0 — the timeline value and ACID transforms

Status: implemented

## Problem

P1.3 (the clip editor) needs an arrangement to edit. Before the render node (P1.3.1)
or the engine log integration (P1.3.2), there must be a *value*: what a clip is, how
clips sit on tracks, and how the ACID operations transform it. This slice delivers that
value and the pure ops, so `Timeline` is a plain, comparable, replayable model the
render node and the engine log can both build on. The design (graph value, one opaque
`ArrangerNode` per track) is the proposed P1.3 shape note; this note records what
shipped.

## Decision

`crates/media/src/timeline.rs` defines the arrangement **value**:

```
Clip     { id, source, src_start, src_len, at_frame, fade_in, fade_out, gain, loop_len }
Track    { id, clips }
Timeline { tracks }
```

- `Frame = u64` (samples); fades are `Frame` too (not `u32`), so they never truncate.
- `Id = String`. Interning to `&'static str` (and canonical `f32`-bit encoding) is a
  **log** concern, deliberately kept out of the value so it stays a plain model.
- Clips are layered (overlaps sum at render) and kept **sorted by `at_frame`** (stable),
  so a render node can binary-search the active window.
- `loop_len: Option<Frame>` makes the source read wrap (a baked loop); a clip is otherwise
  a contiguous window `[src_start, src_start + src_len)` placed at `at_frame`.

`ArrangeOp` is the ACID op set; each **carries the ids it creates**, so ids are logged
and deterministic (never random/UUID/time). `Timeline::apply(&self, op) -> Result<Self>` is
the pure transform; `apply_mut` is the same op validated-then-committed (no partial
mutation on `Err`).

Invariants enforced by every mutating op (fail-loud, never logged on refusal):
`src_len > 0`, `src_len <= i64::MAX` (so signed trim arithmetic never wraps), `gain`
finite, `loop_len != Some(0)`, `at_frame + src_len` not overflowing, and
`fade_in + fade_out <= src_len`. The two sums are **checked, not wrapped**: both fades are
raw `u64` on the `host v1` text path, so a pair can leave the range — see the
[fade-sum fix](../bug-fix/2026-09-29-fade-sums-are-checked-not-wrapped.md).

Opaque loop phase handled explicitly: **`RazorSplit` and `Trim(Start)` refuse a looped
clip** — the loop phase at the cut is not representable in this model, and "drop the
wrap" would silently change audio. This is the kimi design-review must-fix, resolved by
fail-loud rather than a silent audio change.

## Alternatives considered

- **Per-clip nodes** — rejected in the shape note (graph-grow, node-surgery edits, no
  value for undo). The value form is the decision.
- **Serialise the whole `Timeline` into the log per edit** — rejected (the log is a
  command log, not a snapshot store); the ops are the log.
- **Id generation via a hidden counter** — rejected; ids are carried in the ops so the
  log is fully explicit and replay needs no hidden state.
- **`fade_in`/`fade_out` as `u32`** — rejected after the kimi slice-1 review: a
  `u32` cast truncates for `src_len > u32::MAX`; `Frame` keeps the arithmetic honest.
- **Silently dropping `loop_len` on split/trim** — rejected: it changes audio while
  looking clean; the honest choice is a fail-loud refusal until phase is representable.
- **A separate `media` timeline the engine never sees** — rejected (split-brain; replay
  drifts). The ops are logged (P1.3.2 wires the engine dispatch).

## Consequences

- The value alone (an empty timeline) is reproducible from an op log: the
  `pure_apply_is_deterministic_and_replays` test applies the same ops twice and asserts
  equality. This is the byte-identical-replay foundation for the arrangement.
- `Clip::end()` assumes the span was validated on admission; `AddClip` validates the
  embedded `Clip` (a hand-built clip that would overflow/NaN/zero-length is refused on
  the way in), so later ops (`RazorSplit`, `MoveClip`) can trust `at_frame + src_len`.
- `RazorSplit`, `MoveClip`, `MoveClipToTrack`, and `Trim(Start)` all re-sort their track
  (stable, deterministic), so a track's `clips` stays sorted even when a neighbor starts
  *inside* the split clip's span (the kimi slice-1 sortedness bug).
- `LoopRegion` uses `checked_mul` *before* any assignment (the kimi slice-1
  panic/partial-mutation fix); the "refused op is never logged" contract holds because no
  op mutates before its last possible failure point.
- 9 timeline tests pass (sortedness across split/move/trim, loop refusal, validation
  gate, determinism/replay). Workspace: 34 media unit tests, clippy clean.
- **Deferred**: the engine log integration (`Event::Arrangement` + dispatch + `at_frame`)
  is P1.3.2; the render node is P1.3.1; `Trim` currently keeps `at_frame`/`src_len`
  consistent and re-sorts, and a baked-loop *crossfade* at the seam is not represented
  (documented in the shape note).
