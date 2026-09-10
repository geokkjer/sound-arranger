# Agent Note: the timeline viewport — pixels-per-frame zoom and pan

Status: implemented

## Problem

The timeline canvas was a **fit-to-view prototype**: it recomputed `pxPerFrame = width / duration` on
every draw, so the whole arrangement was always squeezed to the window. That is fine for a 1-second
demo and useless for the product's actual material — a 30-minute jam becomes a smear of sub-pixel
clips, and there is no way to zoom in on a cut. There was also no horizontal scroll and no way to see
more than the container's worth of tracks.

## Decision

Implement the viewport model from [`docs/design/ui-plan.md` §3](../../../../docs/design/ui-plan.md):
the timeline is a **fixed time-space viewed through a scrolling window**, so zoom is a mapping change,
never a relayout.

- **State + maths live in `src/timelineView.ts`** (a reactive singleton + pure functions), so the
  canvas and the shell agree on one source of truth: `zoom` (px per frame), `t0` (view start frame),
  `vScroll` (px), `follow`, plus the canvas-published `width`/`height`/`duration`. Functions:
  `fitZoom`, `fitTimeline`, `frameAt`, `xFor`, `zoomAt`, `panBy`, `clampView`, `followPlayhead`.
- **Zoom is anchored at the cursor** (`zoomAt(px, factor)` keeps the frame under the pointer fixed);
  **fit is the zoom-out floor** (you cannot zoom out past edge-to-edge) and there is a px/second
  ceiling. `clampView` also keeps `t0` inside `[0, duration - visible]`, so the view never shows dead
  space past the arrangement.
- **The canvas maps `x = (frame - t0) * zoom`** and **culls**: off-screen lanes and clips are skipped,
  so a long arrangement draws only what is visible and the repaint stays bounded (the DOM never grows
  with clip count — the design's whole reason for a canvas).
- **The ruler's tick step derives from the zoom** (the smallest "nice" step whose labels are ≥64 px
  apart), at *absolute* times, so labels stay stable as you scroll. The same step is what snapping
  will use.
- **Lanes fill the container at fit zoom and grow taller as you zoom in** (ui-plan §3's "one unified
  zoom gesture"), capped at `MAX_LANE_H`, with a vertical scrollbar once they outgrow the viewport.
- **Interactions**: wheel = zoom at the cursor · shift+wheel = pan time · ctrl/cmd+wheel = scroll
  lanes · click = seek. The top bar's **FIT** control now works and a `px/s` readout shows the zoom.
  Loading an arrangement re-fits the view.

## Alternatives considered

- **Stay fit-to-view.** Rejected: unusable for the product's long-form material.
- **Store zoom as pixels-per-second.** Rejected: the canvas maths is in frames; px/s is a display
  unit, and mixing the two invites off-by-a-rate bugs. `px/s` is computed for the readout only.
- **Anchor zoom at the view centre.** Rejected: the design specifies the cursor, which is the correct
  UX (you zoom into what you point at).
- **Allow zooming out past fit** (dead space beyond the last clip) or **past the arrangement edges**.
  Rejected: `clampView` keeps the arrangement edge-to-edge, per §3 ("zoom-out floors at fit").
- **A separate lane-height control.** Rejected for now: §3 says one unified zoom gesture. If it proves
  annoying in use, a modifier can take it over (the state already exists).
- **A DOM scrollbar over the canvas.** Rejected: it would repaint at drag rate through the DOM; the
  scrollbar is drawn on the canvas (and ctrl+wheel scrolls).

## Consequences

- A long arrangement is now navigable: zoom into a cut, pan along it, and the ruler re-scales.
- The snap grid / clips-snap-to-ticks work in `ui-plan` §3 now has its step function in place (the
  ruler's), though snapping itself is not wired (no draggable clips yet).
- **Transport follow** is applied on top: while the transport runs and `follow` is on (the default),
  the canvas calls `followPlayhead(frame)`, which parks the playhead at the left margin once it passes
  the right margin (or sits behind the view). The top bar has a toggle (`⇥`).
- The maths is unit-tested with `vitest` (see the
  [frontend tests note](../process/2026-09-10-frontend-unit-tests-vitest.md)): anchored-zoom
  invariance, the fit floor and the px/s ceiling, `t0` clamping, follow parking, and the ruler ticks.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-10.
