import { reactive } from "vue";

// The timeline viewport — the pixels-per-frame model from docs/design/ui-plan.md
// §3. The timeline is a **fixed time-space viewed through a scrolling window**, so
// zoom is a *mapping change* (`x = (frame - t0) * zoom`), never a per-clip
// relayout: the canvas redraws, the DOM never reflows. This module owns the view
// state and the pure mapping maths, so the canvas and the shell (FIT, follow)
// share one source of truth.

/** The session clock's frames per second — the canvas timebase. */
export const RATE = 48_000;
/** The deepest zoom, in pixels per second (≈6 frames per pixel — plenty for cuts). */
export const MAX_PX_PER_SEC = 8_000;

export const timelineView = reactive({
  /** pixels per frame */
  zoom: 0.01,
  /** the view's start frame (the horizontal scroll offset) */
  t0: 0,
  /** vertical scroll in px (only once the lanes outgrow the viewport) */
  vScroll: 0,
  /** follow the playhead while the transport runs */
  follow: true,
  /** published by the canvas: the container size and the arrangement length */
  width: 0,
  height: 0,
  duration: RATE * 4,
});

/** The zoom that fits the whole arrangement in the viewport — the zoom-out floor. */
export function fitZoom(): number {
  return Math.max(1e-9, timelineView.width) / Math.max(1, timelineView.duration);
}

/** Fit the whole arrangement (`t0 = 0`) — what the top bar's FIT control calls. */
export function fitTimeline(): void {
  timelineView.zoom = fitZoom();
  timelineView.t0 = 0;
  timelineView.vScroll = 0;
}

/** The frame under a viewport x (px). */
export function frameAt(px: number): number {
  return timelineView.t0 + px / timelineView.zoom;
}

/** The viewport x (px) of a frame — may be off-screen. */
export function xFor(frame: number): number {
  return (frame - timelineView.t0) * timelineView.zoom;
}

/**
 * Zoom by `factor` keeping the time under `px` fixed (the correct UX: the frame
 * under the cursor does not move), then clamp to `[fit, max]` and keep the view
 * inside the arrangement.
 */
export function zoomAt(px: number, factor: number): void {
  const v = timelineView;
  const anchor = frameAt(px);
  v.zoom = clamp(v.zoom * factor, fitZoom(), MAX_PX_PER_SEC / RATE);
  v.t0 = anchor - px / v.zoom;
  clampView();
}

/** Pan horizontally by `deltaPx` (positive moves the view later in time). */
export function panBy(deltaPx: number): void {
  timelineView.t0 += deltaPx / timelineView.zoom;
  clampView();
}

/** Clamp zoom to the fit floor/ceiling and `t0` inside `[0, duration - visible]`. */
export function clampView(): void {
  const v = timelineView;
  v.zoom = clamp(v.zoom, fitZoom(), MAX_PX_PER_SEC / RATE);
  const visible = v.width / v.zoom;
  v.t0 = clamp(v.t0, 0, Math.max(0, v.duration - visible));
}

/**
 * Keep the playhead in view while following: once it passes the right margin (or
 * sits behind the view), park it at the left margin and let it run again.
 */
export function followPlayhead(frame: number): void {
  const v = timelineView;
  if (!v.follow) return;
  const margin = v.width * 0.15;
  const x = xFor(frame);
  if (x < 0 || x > v.width - margin) {
    v.t0 = frame - margin / v.zoom;
    clampView();
  }
}

function clamp(n: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, n));
}
