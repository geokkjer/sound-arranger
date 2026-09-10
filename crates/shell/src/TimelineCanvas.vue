<script setup lang="ts">
// The timeline canvas — the clip-arranger's hero ui-plugin. It draws the
// arrangement value through the **viewport** (zoom · t0), the live playhead, and
// it is the editor: click to select, drag to move, drag an edge to resize, drag
// empty space to marquee-select, R to razor at the playhead, Delete to remove.
//
// The timeline is a fixed time-space viewed through a scrolling window: zoom is a
// mapping change (`x = (frame - t0) * zoom`), so nothing relayouts — only the
// canvas repaints, and clips outside the window are culled. Gestures preview
// locally and commit **one op** on release (the bridge applies a single arrange
// op); the pure decisions live in ./timelineEdit and ./timelineView.

import { onMounted, onUnmounted, ref, watch } from "vue";
import { bridgeState, type Clip, type Timeline } from "./bridge";
import { applyArrange, loadScript } from "./editor";
import { transportSeek } from "./transport";
import { tickLabel, tickStepFrames } from "./timelineTicks";
import {
  type Hit,
  clipAt,
  freshIds,
  hitTest,
  laneAt,
  maxSrcLen,
  opDelete,
  opMove,
  opMoveToTrack,
  opRazorSplit,
  opTrimEnd,
  opTrimStart,
  snapContext,
  snapFrame,
  snapMove,
} from "./timelineEdit";
import {
  RATE,
  clampView,
  fitTimeline,
  fitZoom,
  followPlayhead,
  frameAt,
  panBy,
  timelineView as view,
  xFor,
  zoomAt,
} from "./timelineView";

// The default script is the synth chain (euclidean → scale → tone → mixer), so
// "Run in host" then ▶ makes sound with no pool/recording needed. It must be a
// <textarea>: an <input> strips newlines from its value, collapsing the script
// into one line and failing the `host v1` header check.
const hostScript = ref(
  [
    "host v1",
    "mount euclidean steps=8 pulses=5 @0",
    "mount scale root=0 note_len=2400 @0",
    "mount tone gain=0.9 blip_len=1800 @0",
    "mount mixer channels=4 @0",
    "patch euclidean.triggers scale.trigger @0",
    "patch scale.note tone.note @0",
    "patch tone.audio mixer.ch0 @0",
  ].join("\n"),
);
const status = ref("drawing demo arrangement");

// A demo arrangement (frames = samples at 48 kHz) so the canvas is meaningful
// without a pool. 48000 frames = 1 second.
const demo: Timeline = {
  tracks: [
    { id: "t0", clips: [
      { id: "c0", source: "s1", src_start: 0, src_len: 12000, at_frame: 0, fade_in: 0, fade_out: 2000, gain: 1.0, loop_len: null },
      { id: "c1", source: "s2", src_start: 0, src_len: 24000, at_frame: 24000, fade_in: 1000, fade_out: 0, gain: 0.8, loop_len: null },
    ]},
    { id: "t1", clips: [
      { id: "c2", source: "s3", src_start: 0, src_len: 6000, at_frame: 48000, fade_in: 0, fade_out: 0, gain: 0.5, loop_len: 2000 },
    ]},
  ],
};

const canvas = ref<HTMLCanvasElement | null>(null);
// The arrangement currently drawn (the loaded/edited one, or the demo).
const drawn = ref<Timeline>(demo);
// The selected clip ids (the accent border).
const selected = ref<Set<string>>(new Set());

// Per-track identity colors (mirror the --track-N tokens in style.css). A track
// keeps one hue across its lane swatch, its clips (and, later, its mixer strip).
const TRACK_COLORS = ["#4fa3ad", "#5aa46a", "#d0723a", "#b45c8c", "#8a6bb0", "#c3a84a", "#6d9a77", "#9a6d7a"];
const ACCENT = "#6b8afd";

/** Geometry (px): the ruler is a fixed strip; lanes fill it at fit zoom. */
const RULER_H = 20;
const MAX_LANE_H = 220;

const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));

/** The lane height for the current zoom: fill at fit, grow as you zoom in. */
function laneHeightOf(tracks: number, height: number): number {
  const fill = (height - RULER_H) / Math.max(1, tracks);
  return Math.max(fill, Math.min(fill * (view.zoom / fitZoom()), MAX_LANE_H));
}

/** The arrangement's length in frames (a 4 s default window when empty). */
function durationFrames(tl: Timeline): number {
  let max = 0;
  for (const t of tl.tracks) {
    for (const c of t.clips) max = Math.max(max, c.at_frame + c.src_len);
  }
  return max > 0 ? max : RATE * 4;
}

/** The source's frame count, when the pool listing has it (bounds a resize). */
function sourceFramesOf(clip: Clip): number | undefined {
  return bridgeState.sources.find((s) => s.id === clip.source)?.frames;
}

// ---- the drag state (a plain local: only draw() reads it) -------------------

interface Drag {
  kind: "move" | "start" | "end" | "marquee";
  hit: Hit | null;
  startX: number;
  startY: number;
  orig: { at: number; len: number; start: number; track: number };
  preview: { at: number; len: number; track: number };
  marquee: { x0: number; y0: number; x1: number; y1: number } | null;
  moved: boolean;
}
let drag: Drag | null = null;

function draw() {
  const c = canvas.value;
  if (!c) return;
  const ctx = c.getContext("2d");
  if (!ctx) return;
  const w = c.clientWidth, h = c.clientHeight;
  if (w === 0 || h === 0) return;
  const dpr = window.devicePixelRatio || 1;
  if (c.width !== Math.round(w * dpr) || c.height !== Math.round(h * dpr)) {
    c.width = Math.round(w * dpr);
    c.height = Math.round(h * dpr);
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);

  const tl = drawn.value;
  // Publish the viewport, then clamp the view (fit floor, inside the arrangement).
  view.width = w;
  view.height = h;
  view.duration = durationFrames(tl);
  clampView();

  const z = view.zoom;
  const x0 = view.t0;
  const tracks = Math.max(1, tl.tracks.length);
  const laneH = laneHeightOf(tracks, h);
  const viewH = h - RULER_H;
  view.vScroll = clamp(view.vScroll, 0, Math.max(0, tracks * laneH - viewH));

  ctx.fillStyle = "#14161a";
  ctx.fillRect(0, 0, w, h);

  // lanes + clips — culled, so a 30-minute arrangement draws only what is on screen
  tl.tracks.forEach((track, ti) => {
    const y = RULER_H - view.vScroll + ti * laneH;
    if (y >= h || y + laneH <= RULER_H) return;
    ctx.fillStyle = "#1d2026";
    ctx.fillRect(0, y, w, laneH - 4);
    ctx.fillStyle = "#6b7280";
    ctx.font = "11px ui-monospace, monospace";
    ctx.fillText(track.id, 8, y + 14);
    const trackColor = TRACK_COLORS[ti % TRACK_COLORS.length];
    ctx.fillStyle = trackColor;
    ctx.fillRect(0, y, 3, laneH - 4);
    track.clips.forEach((clip, ci) => {
      // the clip being dragged is drawn as a preview after the lanes (below)
      if (drag && drag.moved && drag.hit && drag.hit.track === ti && drag.hit.clip === ci) return;
      const x = (clip.at_frame - x0) * z;
      const wpx = clip.src_len * z;
      if (x >= w || x + wpx <= 0) return; // off-screen
      drawClip(ctx, trackColor, clip.id, x, y, wpx, laneH, selected.value.has(clip.id));
    });
  });

  // the drag preview (move or resize): the dragged clip at its preview geometry
  if (drag && drag.moved && drag.hit && drag.kind !== "marquee") {
    const track = tl.tracks[drag.preview.track];
    if (track) {
      const y = RULER_H - view.vScroll + drag.preview.track * laneH;
      const x = (drag.preview.at - x0) * z;
      const wpx = drag.preview.len * z;
      const color = TRACK_COLORS[drag.preview.track % TRACK_COLORS.length];
      drawClip(ctx, color, clipIdOf(drag.hit), x, y, wpx, laneH, true);
    }
  }

  // marquee selection rectangle
  if (drag?.marquee) {
    const { x0: mx0, y0: my0, x1: mx1, y1: my1 } = drag.marquee;
    const rx = Math.min(mx0, mx1), ry = Math.min(my0, my1);
    const rw = Math.abs(mx1 - mx0), rh = Math.abs(my1 - my0);
    ctx.fillStyle = "rgba(107,138,253,0.10)";
    ctx.fillRect(rx, ry, rw, rh);
    ctx.strokeStyle = ACCENT;
    ctx.lineWidth = 1;
    ctx.strokeRect(rx + 0.5, ry + 0.5, rw, rh);
  }

  // ruler — ticks at absolute times, their step derived from the zoom
  ctx.fillStyle = "#17191f";
  ctx.fillRect(0, 0, w, RULER_H);
  ctx.strokeStyle = "#22252c";
  ctx.beginPath();
  ctx.moveTo(0, RULER_H - 0.5);
  ctx.lineTo(w, RULER_H - 0.5);
  ctx.stroke();
  const step = tickStepFrames(z);
  ctx.font = "10px ui-monospace, monospace";
  for (let f = Math.floor(x0 / step) * step; ; f += step) {
    const x = (f - x0) * z;
    if (x > w) break;
    if (x < -1) continue;
    ctx.strokeStyle = "#2b313a";
    ctx.beginPath();
    ctx.moveTo(Math.round(x) + 0.5, RULER_H - 5);
    ctx.lineTo(Math.round(x) + 0.5, RULER_H);
    ctx.stroke();
    ctx.fillStyle = "#8891a5";
    ctx.fillText(tickLabel(f), x + 3, 11);
  }

  // vertical scrollbar — appears only once the lanes outgrow the viewport
  const totalH = tracks * laneH;
  if (totalH > viewH + 1) {
    const trackW = 3;
    ctx.fillStyle = "#1d2026";
    ctx.fillRect(w - trackW, RULER_H, trackW, viewH);
    const thumbH = Math.max(12, (viewH / totalH) * viewH);
    const thumbY = RULER_H + (view.vScroll / (totalH - viewH)) * (viewH - thumbH);
    ctx.fillStyle = "#3a424e";
    ctx.fillRect(w - trackW, thumbY, trackW, thumbH);
  }

  // playhead — the live transport position (amber, per the design system)
  const px = xFor(bridgeState.position.frame);
  if (px >= 0 && px <= w) {
    ctx.strokeStyle = "#e5b567";
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.moveTo(px + 0.5, 0);
    ctx.lineTo(px + 0.5, h);
    ctx.stroke();
    ctx.fillStyle = "#e5b567";
    ctx.beginPath();
    ctx.moveTo(px - 4, 0);
    ctx.lineTo(px + 4, 0);
    ctx.lineTo(px, 6);
    ctx.closePath();
    ctx.fill();
  }
}

function clipIdOf(hit: Hit): string {
  return drawn.value.tracks[hit.track]?.clips[hit.clip]?.id ?? "";
}

/** One clip block: the track's hue, an accent border when selected. */
function drawClip(
  ctx: CanvasRenderingContext2D,
  color: string,
  id: string,
  x: number,
  y: number,
  wpx: number,
  laneH: number,
  isSelected: boolean,
): void {
  ctx.fillStyle = color;
  ctx.fillRect(x, y + 4, Math.max(2, wpx), laneH - 12);
  ctx.strokeStyle = isSelected ? ACCENT : "rgba(255,255,255,0.35)";
  ctx.lineWidth = isSelected ? 2 : 1;
  ctx.strokeRect(x, y + 4, Math.max(2, wpx), laneH - 12);
  ctx.lineWidth = 1;
  if (wpx > 24) {
    ctx.fillStyle = "#0b0d10";
    ctx.font = "10px ui-monospace, monospace";
    ctx.fillText(id, x + 4, y + 16);
  }
}

// ---- gestures --------------------------------------------------------------

function canvasXY(e: PointerEvent | MouseEvent): { x: number; y: number } {
  const rect = canvas.value!.getBoundingClientRect();
  return { x: e.clientX - rect.left, y: e.clientY - rect.top };
}

/** The clip id under the pointer, or null — for the hover cursor. */
function hitAtPointer(x: number, y: number): Hit | null {
  const c = canvas.value;
  if (!c) return null;
  const tracks = Math.max(1, drawn.value.tracks.length);
  const ti = laneAt(y, tracks, RULER_H, laneHeightOf(tracks, c.clientHeight), view.vScroll);
  return ti === null ? null : hitTest(drawn.value, ti, frameAt(x), view.zoom);
}

function onPointerDown(e: PointerEvent) {
  const c = canvas.value;
  if (!c || e.button !== 0) return;
  c.focus();
  const { x, y } = canvasXY(e);
  const hit = hitAtPointer(x, y);

  if (hit) {
    const clip = clipAt(drawn.value, hit)!;
    if (!selected.value.has(clip.id)) selected.value = new Set([clip.id]);
    drag = {
      kind: hit.zone === "body" ? "move" : hit.zone,
      hit,
      startX: x,
      startY: y,
      orig: { at: clip.at_frame, len: clip.src_len, start: clip.src_start, track: hit.track },
      preview: { at: clip.at_frame, len: clip.src_len, track: hit.track },
      marquee: null,
      moved: false,
    };
  } else {
    if (!e.shiftKey) selected.value = new Set();
    drag = {
      kind: "marquee",
      hit: null,
      startX: x,
      startY: y,
      orig: { at: 0, len: 0, start: 0, track: 0 },
      preview: { at: 0, len: 0, track: 0 },
      marquee: { x0: x, y0: y, x1: x, y1: y },
      moved: false,
    };
  }
  try {
    c.setPointerCapture(e.pointerId);
  } catch {
    // capture is best-effort (a synthetic pointer may not be capturable)
  }
  draw();
}

function onPointerMove(e: PointerEvent) {
  const c = canvas.value;
  if (!c) return;
  const { x, y } = canvasXY(e);

  if (!drag) {
    // hover affordance: move over a body, resize over an edge
    const hit = hitAtPointer(x, y);
    c.style.cursor = hit ? (hit.zone === "body" ? "move" : "ew-resize") : "crosshair";
    return;
  }

  if (Math.abs(x - drag.startX) > 2 || Math.abs(y - drag.startY) > 2) drag.moved = true;

  if (drag.kind === "marquee") {
    drag.marquee = { x0: drag.startX, y0: drag.startY, x1: x, y1: y };
    draw();
    return;
  }

  const tracks = Math.max(1, drawn.value.tracks.length);
  const laneH = laneHeightOf(tracks, c.clientHeight);
  const ctx = snapContext(drawn.value, view.zoom, drag.hit ?? undefined);
  const delta = frameAt(x) - frameAt(drag.startX);

  if (drag.kind === "move") {
    drag.preview.at = snapMove(drag.orig.len, Math.max(0, drag.orig.at + delta), ctx);
    const ti = laneAt(y, tracks, RULER_H, laneH, view.vScroll);
    drag.preview.track = ti ?? drag.orig.track;
  } else if (drag.kind === "end") {
    const clip = clipAt(drawn.value, drag.hit!);
    const maxLen = clip ? maxSrcLen(sourceFramesOf(clip), clip.src_start) : undefined;
    const end = snapFrame(drag.orig.at + Math.max(1, drag.orig.len + delta), ctx);
    const len = Math.max(1, end - drag.orig.at);
    drag.preview.len = maxLen === undefined ? len : Math.min(len, maxLen);
  } else if (drag.kind === "start") {
    const minAt = Math.max(0, drag.orig.at - drag.orig.start);
    const maxAt = drag.orig.at + drag.orig.len - 1;
    drag.preview.at = clamp(snapFrame(Math.max(0, drag.orig.at + delta), ctx), minAt, maxAt);
  }
  draw();
}

async function onPointerUp(e: PointerEvent) {
  const c = canvas.value;
  const d = drag;
  drag = null;
  if (!c || !d) return;
  c.releasePointerCapture?.(e.pointerId);

  if (d.kind === "marquee") {
    if (d.moved && d.marquee) {
      selectInMarquee(d.marquee);
    } else {
      // a plain click on empty space still places the playhead (snapped)
      const { x } = canvasXY(e);
      const ctx = snapContext(drawn.value, view.zoom);
      transportSeek(Math.max(0, Math.round(snapFrame(frameAt(x), ctx)))).catch(
        (err) => (bridgeState.status = String(err)),
      );
    }
    draw();
    return;
  }

  if (!d.moved || !d.hit) {
    draw();
    return;
  }
  const trackId = drawn.value.tracks[d.hit.track]?.id ?? "";
  const clipId = clipIdOf(d.hit);
  try {
    if (d.kind === "move") {
      const toId = drawn.value.tracks[d.preview.track]?.id ?? trackId;
      const line = d.preview.track === d.hit.track
        ? opMove(trackId, clipId, d.preview.at)
        : opMoveToTrack(trackId, clipId, toId, d.preview.at);
      await applyArrange(line);
      bridgeState.status = `moved ${clipId}`;
    } else if (d.kind === "end") {
      const orig = drawn.value.tracks[d.hit.track]?.clips[d.hit.clip];
      if (orig) {
        await applyArrange(opTrimEnd(trackId, clipId, d.preview.len - orig.src_len));
        bridgeState.status = `resized ${clipId}`;
      }
    } else if (d.kind === "start") {
      const orig = drawn.value.tracks[d.hit.track]?.clips[d.hit.clip];
      if (orig) {
        await applyArrange(opTrimStart(trackId, clipId, d.preview.at - orig.at_frame));
        bridgeState.status = `resized ${clipId}`;
      }
    }
  } catch (err) {
    bridgeState.status = String(err);
  }
  draw();
}

/** Select every clip the marquee rectangle overlaps (time × lane). */
function selectInMarquee(m: { x0: number; y0: number; x1: number; y1: number }) {
  const c = canvas.value;
  if (!c) return;
  const tracks = Math.max(1, drawn.value.tracks.length);
  const laneH = laneHeightOf(tracks, c.clientHeight);
  const f0 = frameAt(Math.min(m.x0, m.x1));
  const f1 = frameAt(Math.max(m.x0, m.x1));
  const t0 = laneAt(Math.min(m.y0, m.y1), tracks, RULER_H, laneH, view.vScroll) ?? 0;
  const t1 = laneAt(Math.max(m.y0, m.y1), tracks, RULER_H, laneH, view.vScroll) ?? tracks - 1;
  const next = new Set<string>();
  for (let ti = Math.min(t0, t1); ti <= Math.max(t0, t1); ti++) {
    for (const clip of drawn.value.tracks[ti]?.clips ?? []) {
      if (clip.at_frame < f1 && clip.at_frame + clip.src_len > f0) next.add(clip.id);
    }
  }
  selected.value = next;
}

/** Delete (Delete/Backspace) and razor at the playhead (R); Escape clears. */
async function onKeyDown(e: KeyboardEvent) {
  if (e.key === "Escape") {
    selected.value = new Set();
    draw();
    return;
  }
  if (selected.value.size === 0) return;
  const tl = drawn.value;
  const targets: Array<{ track: string; clip: Clip }> = [];
  tl.tracks.forEach((t) => t.clips.forEach((c) => {
    if (selected.value.has(c.id)) targets.push({ track: t.id, clip: c });
  }));

  if (e.key === "Delete" || e.key === "Backspace") {
    e.preventDefault();
    for (const { track, clip } of targets) {
      try {
        await applyArrange(opDelete(track, clip.id));
      } catch (err) {
        bridgeState.status = String(err);
      }
    }
    selected.value = new Set();
    bridgeState.status = `deleted ${targets.length} clip(s)`;
  } else if (e.key === "r" || e.key === "R") {
    e.preventDefault();
    const ctx = snapContext(tl, view.zoom);
    const cut = Math.round(snapFrame(bridgeState.position.frame, ctx));
    for (const { track, clip } of targets) {
      const end = clip.at_frame + clip.src_len;
      if (cut <= clip.at_frame || cut >= end) {
        bridgeState.status = `razor: put the playhead inside ${clip.id} first`;
        continue;
      }
      if (clip.loop_len !== null) {
        bridgeState.status = `razor: ${clip.id} is a looped clip (its phase is not representable)`;
        continue;
      }
      const [left, right] = freshIds(tl, clip.id, 2);
      try {
        await applyArrange(opRazorSplit(track, clip.id, left, right, cut));
        selected.value = new Set([left, right]);
        bridgeState.status = `split ${clip.id} at ${cut}`;
      } catch (err) {
        bridgeState.status = String(err);
      }
    }
  }
  draw();
}

/**
 * The wheel is the one zoom gesture (ui-plan §3): plain wheel zooms around the
 * cursor, shift+wheel pans in time, ctrl/cmd+wheel scrolls the lanes.
 */
function onWheel(e: WheelEvent) {
  e.preventDefault();
  const c = canvas.value;
  if (!c) return;
  const rect = c.getBoundingClientRect();
  if (e.ctrlKey || e.metaKey) {
    view.vScroll = Math.max(0, view.vScroll + e.deltaY);
  } else if (e.shiftKey) {
    panBy(e.deltaY);
  } else {
    zoomAt(e.clientX - rect.left, Math.exp(-e.deltaY * 0.0015));
  }
  // The view watcher redraws (batched per tick, so a fast wheel is one repaint).
}

async function loadFromHost() {
  try {
    status.value = "loading into the live host…";
    const outcome = await loadScript(hostScript.value);
    if (outcome.arrangement && outcome.arrangement.tracks.length > 0) {
      drawn.value = outcome.arrangement;
      status.value = `loaded ${outcome.arrangement.tracks.length} track(s)`;
    } else {
      drawn.value = demo;
      status.value = "no arrangement tracks — showing the demo (the graph still plays)";
    }
    selected.value = new Set();
    bridgeState.status = status.value;
    // Frame the new arrangement: fit it (the load may be much longer/shorter).
    view.duration = durationFrames(drawn.value);
    fitTimeline();
    draw();
  } catch (e) {
    status.value = String(e);
    bridgeState.status = String(e);
    bridgeState.lastError = String(e);
    drawn.value = demo;
    draw();
  }
}

// Redraw the playhead whenever the polled position changes; while the transport
// runs, follow it (auto-scroll) when the user has follow on.
watch(
  () => bridgeState.position.frame,
  (frame) => {
    if (bridgeState.position.playing) followPlayhead(frame);
    draw();
  },
);
// An edit (or a load) replaces the arrangement value — draw it.
watch(
  () => bridgeState.arrangement,
  (tl) => {
    drawn.value = tl && tl.tracks.length > 0 ? tl : demo;
    draw();
  },
);
// Redraw when the shell changes the view (FIT, follow), not for width/duration
// (draw sets those — watching them would loop).
watch(() => [view.zoom, view.t0, view.vScroll], () => draw());

onMounted(() => {
  draw();
  window.addEventListener("resize", draw);
  canvas.value?.addEventListener("wheel", onWheel, { passive: false });
});
onUnmounted(() => {
  window.removeEventListener("resize", draw);
  canvas.value?.removeEventListener("wheel", onWheel);
});
</script>

<template>
  <div class="timeline">
    <div class="tl-bar">
      <span class="tl-status">{{ status }}</span>
      <textarea
        v-model="hostScript"
        class="tl-script"
        rows="1"
        spellcheck="false"
        placeholder="host v1 — one command per line"
        title="the host script (one command per line; drag to expand)"
      ></textarea>
      <button class="btn" @click="loadFromHost">Run in host</button>
    </div>
    <canvas
      ref="canvas"
      class="tl-canvas"
      tabindex="0"
      title="click: select/seek · drag: move · drag an edge: resize · drag empty: marquee · R: razor at the playhead · Delete: remove · wheel: zoom · shift+wheel: pan · ctrl+wheel: scroll lanes"
      @pointerdown="onPointerDown"
      @pointermove="onPointerMove"
      @pointerup="onPointerUp"
      @pointercancel="onPointerUp"
      @keydown="onKeyDown"
    ></canvas>
  </div>
</template>

<style scoped>
.timeline { display: flex; flex-direction: column; height: 100%; min-height: 0; }
.tl-bar { display: flex; gap: 8px; align-items: center; padding: 8px; background: #17191f; border-bottom: 1px solid #22252c; }
.tl-status { font: 11px ui-monospace, monospace; color: #8b93a7; white-space: nowrap; max-width: 40%; overflow: hidden; text-overflow: ellipsis; }
.tl-script {
  flex: 1;
  font: 11px ui-monospace, monospace;
  color: #cfd6e4;
  background: #0e1013;
  border: 1px solid #262a32;
  border-radius: 4px;
  padding: 4px 6px;
  /* a <textarea> keeps the newlines an <input> would strip; drag to expand */
  resize: vertical;
  min-height: 22px;
  max-height: 40vh;
  line-height: 1.35;
  white-space: pre;
  overflow: auto;
}
.tl-canvas { flex: 1; width: 100%; min-height: 0; display: block; cursor: crosshair; outline: none; }
</style>
