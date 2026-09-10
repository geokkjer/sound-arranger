<script setup lang="ts">
// The timeline canvas — the clip-arranger's hero ui-plugin. It draws the
// arrangement value (tracks → clips) through the **viewport** (zoom · t0), the
// live playhead, and the seek gesture. "Run in host" loads a script into the live
// session (reset-and-apply), so the arrangement, meters and transport come from
// the engine.
//
// The timeline is a fixed time-space viewed through a scrolling window: zoom is a
// *mapping change* (`x = (frame - t0) * zoom`), so nothing relayouts — only the
// canvas repaints, and clips outside the window are culled. The view state and
// maths live in ./timelineView (shared with the shell's FIT / follow).

import { invoke } from "@tauri-apps/api/core";
import { onMounted, onUnmounted, ref, watch } from "vue";
import { bridgeState, type PoolSource, type Timeline } from "./bridge";
import { transportSeek } from "./transport";
import {
  RATE,
  clampView,
  fitTimeline,
  fitZoom,
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
// The arrangement currently drawn (the loaded one, or the demo).
const drawn = ref<Timeline>(demo);

// Per-track identity colors (mirror the --track-N tokens in style.css). A track
// keeps one hue across its lane swatch, its clips (and, later, its mixer strip).
const TRACK_COLORS = ["#4fa3ad", "#5aa46a", "#d0723a", "#b45c8c", "#8a6bb0", "#c3a84a", "#6d9a77", "#9a6d7a"];

/** Geometry (px): the ruler is a fixed strip; lanes fill it at fit zoom. */
const RULER_H = 20;
const MAX_LANE_H = 220;
/** A ruler tick's label needs at least this much room. */
const MIN_TICK_PX = 64;
/** "Nice" tick steps, in seconds. */
const NICE_SECONDS = [0.05, 0.1, 0.25, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600];

const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));

/** The arrangement's length in frames (a 4 s default window when empty). */
function durationFrames(tl: Timeline): number {
  let max = 0;
  for (const t of tl.tracks) {
    for (const c of t.clips) max = Math.max(max, c.at_frame + c.src_len);
  }
  return max > 0 ? max : RATE * 4;
}

/** The smallest "nice" tick step whose labels are at least MIN_TICK_PX apart. */
function tickStepFrames(zoom: number): number {
  const pxPerSec = zoom * RATE;
  const sec = NICE_SECONDS.find((s) => s * pxPerSec >= MIN_TICK_PX) ?? NICE_SECONDS[NICE_SECONDS.length - 1];
  return sec * RATE;
}

function tickLabel(frame: number): string {
  const s = frame / RATE;
  if (s >= 60) {
    const m = Math.floor(s / 60);
    return `${m}:${Math.round(s - m * 60).toString().padStart(2, "0")}`;
  }
  return s < 1 ? `${s.toFixed(2)}s` : `${Number.isInteger(s) ? s : s.toFixed(1)}s`;
}

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
  // One unified zoom gesture: lanes fill the height at fit zoom and grow taller as
  // you zoom in (ui-plan §3), scrollable once they outgrow the viewport.
  const fillLaneH = (h - RULER_H) / tracks;
  // Lanes **fill** the container at fit zoom; they grow taller as you zoom in
  // (one unified zoom gesture, ui-plan §3), capped so they stay readable.
  const laneH = Math.max(fillLaneH, Math.min(fillLaneH * (z / fitZoom()), MAX_LANE_H));
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
    // per-track color: a 3px swatch at the lane's left edge (the header color).
    const trackColor = TRACK_COLORS[ti % TRACK_COLORS.length];
    ctx.fillStyle = trackColor;
    ctx.fillRect(0, y, 3, laneH - 4);
    for (const clip of track.clips) {
      const x = (clip.at_frame - x0) * z;
      const wpx = clip.src_len * z;
      if (x >= w || x + wpx <= 0) continue; // off-screen
      ctx.fillStyle = trackColor;
      ctx.fillRect(x, y + 4, Math.max(2, wpx), laneH - 12);
      ctx.strokeStyle = "rgba(255,255,255,0.35)";
      ctx.strokeRect(x, y + 4, Math.max(2, wpx), laneH - 12);
      if (wpx > 24) {
        ctx.fillStyle = "#0b0d10";
        ctx.font = "10px ui-monospace, monospace";
        ctx.fillText(clip.id, x + 4, y + 16);
      }
    }
  });

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

/** A click sets the playhead (seek to the frame under the pointer). */
function seekFromClick(e: MouseEvent) {
  const c = canvas.value;
  if (!c) return;
  const rect = c.getBoundingClientRect();
  const frame = Math.max(0, Math.round(frameAt(e.clientX - rect.left)));
  transportSeek(frame).catch((err) => (bridgeState.status = String(err)));
}

async function loadFromHost() {
  try {
    status.value = "loading into the live host…";
    const outcome = await invoke<{
      arrangement: Timeline | null;
      arrangement_error: string | null;
      mixer_meters: { channels: number[]; master: number } | null;
      pool_sources: PoolSource[] | null;
      position: { frame: number; seconds: number; beat: number; bpm: number; playing: boolean };
      channel_count: number;
    }>("run_host_script", { scriptText: hostScript.value });
    bridgeState.meters = outcome.mixer_meters;
    bridgeState.sources = outcome.pool_sources ?? [];
    bridgeState.arrangement = outcome.arrangement;
    bridgeState.position = outcome.position;
    bridgeState.channelCount = outcome.channel_count;
    bridgeState.lastError = outcome.arrangement_error;
    if (outcome.arrangement && outcome.arrangement.tracks.length > 0) {
      drawn.value = outcome.arrangement;
      status.value = `loaded ${outcome.arrangement.tracks.length} track(s)`;
    } else {
      drawn.value = demo;
      status.value = "no arrangement tracks — showing the demo (the graph still plays)";
    }
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

// Redraw the playhead whenever the polled position changes.
watch(() => bridgeState.position.frame, () => draw());
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
      title="wheel: zoom · shift+wheel: pan · ctrl+wheel: scroll lanes · click: seek"
      @click="seekFromClick"
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
.tl-canvas { flex: 1; width: 100%; min-height: 0; display: block; cursor: crosshair; }
</style>
