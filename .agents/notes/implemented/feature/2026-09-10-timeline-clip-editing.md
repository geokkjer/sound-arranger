# Agent Note: timeline clip editing — select, move, resize, razor

Status: implemented

## Problem

The timeline canvas was **read-only**: it drew the arrangement, the playhead and the viewport, but
there was no selection, no drag, no razor — so the tool could not actually arrange anything. The
incremental arrange seam existed ([note](2026-09-10-bridge-incremental-arrange.md)); nothing used it.

## Decision

- **Pure decisions in `src/timelineEdit.ts`** (15 unit tests): `laneAt` (y → track), `hitTest`
  (pointer → clip + `body`/`start`/`end`, with the edge grab a *pixel* width converted by `zoom` so it
  stays ~6 px at any zoom), snapping (`snapContext` = the ruler's grid step + every *other* clip's
  edges; `snapFrame` for a cut; `snapMove` which tries both edges and takes whichever is nearer a
  target), `freshIds` (deterministic new ids for split/duplicate), `maxSrcLen`, and the op-line
  builders for the text grammar.
- **One bridge client in `src/editor.ts`** (`loadScript`, `applyArrange`) sharing one `foldOutcome`,
  so a load and an edit update every panel (timeline, mixer, pool) identically.
- **Gestures** in the canvas: click a clip selects it (empty click clears and **seeks**, snapped);
  drag a body **moves** (across lanes → `move_clip_to_track`); drag an edge **resizes** (`trim start`
  / `trim end`, clamped to the source's length when the pool knows it and to ≥1 frame); drag empty
  space **marquee-selects** (time × lane overlap); **R** razors the selected clip at the playhead
  (snapped); **Delete/Backspace** removes; **Escape** clears.
- **A gesture previews locally and commits one op on release.** The drag mutates only a local preview
  (drawn as a lifted clip / selection rectangle); the release sends a single op line. That is why the
  op path is per-gesture and not per-frame — the log gets one entry per intentional edit.
- Refusals (a razor outside the clip, a looped clip, an unknown id) surface in the status line; the
  engine's validate-first rule means nothing changes.

## Alternatives considered

- **Commit an op per pointermove** (or debounce). Rejected: the IPC and the log would fill with
  intermediate positions, and the log's own notion is a coalesced gesture. Preview locally, commit
  once.
- **A DOM/SVG clip layer over the canvas.** Rejected: the design's model is canvas, and DOM-per-clip
  grows with the arrangement (the viewport already culls in canvas).
- **Move by re-adding the clip** (`delete` + `add_clip`) instead of `move_clip`. Rejected: two log
  entries for one gesture, and it would need a fresh id.
- **Pushing the whole arrangement value back to the engine.** Rejected: the engine edits are ops
  (logged, replayable); the value flows engine → shell, never back.
- **Live loop-span handles.** **Not implemented, and not implementable yet**: the engine's clip has a
  *baked* `loop_len` (a repeated read, `src_len = r * times`), not the design's live
  `loop_in`/`loop_out` + fill rule, so there is no loop span to drag. The clip model has to grow
  first (an engine step, not a UI one). Today a resize is a **trim** (a contiguous read).
- **Undo.** Not in this unit: the log is append-only and the host has no inverse-op or snapshot
  history. It is the next piece.

## Consequences

- The timeline is an editor: select, move, resize, split, delete — one op per gesture, each landing in
  the session log (so a seek-rebuild keeps the edits, since they are part of the reconstructed
  session).
- Resize is trim (no loop fill); razor refuses a looped clip with an explanation; there are no loop
  handles. These are all consequences of the clip model, recorded above.
- Undo/redo (and the top bar's disabled buttons) is the next milestone.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-10.
