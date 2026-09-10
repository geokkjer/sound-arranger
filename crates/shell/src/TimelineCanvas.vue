<script setup lang="ts">
// The timeline canvas — the first real ui-plugin of the clip-arranger profile.
// It draws the arrangement value (tracks → clips) as blocks on a frame-scaled
// timeline, draws the **live playhead** at the polled transport position, and
// lets a click seek. "Run in host" loads a script into the live session
// (reset-and-apply), so the arrangement, meters and transport all come from the
// engine.
//
// Fit-to-view: the whole arrangement is scaled to the canvas width, so the
// playhead stays visible and a click maps cleanly to a frame. (The pixels-per-
// second viewport / zoom model from docs/design/ui-plan.md is a later step.)

import { invoke } from "@tauri-apps/api/core";
import { onMounted, onUnmounted, ref, watch } from "vue";
import { bridgeState, type PoolSource, type Timeline } from "./bridge";
import { transportSeek } from "./transport";

const hostScript = ref(
  "host v1\nmount mixer channels=4 @0\narrange add_track t0 @0\narrange add_track t1 @0",
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
let pxPerFrame = 0.01; // recomputed on every draw (fit-to-view)

// Per-track identity colors (mirror the --track-N tokens in style.css). A track
// keeps one hue across its lane swatch, its clips (and, later, its mixer strip).
const TRACK_COLORS = ["#4fa3ad", "#5aa46a", "#d0723a", "#b45c8c", "#8a6bb0", "#c3a84a", "#6d9a77", "#9a6d7a"];
const RATE = 48_000;

/** The arrangement's length in frames (a 4 s default window when empty). */
function durationFrames(tl: Timeline): number {
  let max = 0;
  for (const t of tl.tracks) {
    for (const c of t.clips) max = Math.max(max, c.at_frame + c.src_len);
  }
  return max > 0 ? max : RATE * 4;
}

function draw() {
  const c = canvas.value;
  if (!c) return;
  const ctx = c.getContext("2d");
  if (!ctx) return;
  const tl = drawn.value;
  const dpr = window.devicePixelRatio || 1;
  const w = c.clientWidth, h = c.clientHeight;
  if (c.width !== Math.round(w * dpr) || c.height !== Math.round(h * dpr)) {
    c.width = Math.round(w * dpr);
    c.height = Math.round(h * dpr);
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);

  ctx.fillStyle = "#14161a";
  ctx.fillRect(0, 0, w, h);

  // Fit the whole arrangement to the width (2 px right margin) so the playhead
  // is always on screen and a click maps linearly to a frame.
  pxPerFrame = Math.max(1e-6, (w - 2) / durationFrames(tl));

  const laneH = Math.max(48, (h - 20) / Math.max(1, tl.tracks.length));
  tl.tracks.forEach((track, ti) => {
    const y = 20 + ti * laneH;
    // lane background + label
    ctx.fillStyle = "#1d2026";
    ctx.fillRect(0, y, w, laneH - 4);
    ctx.fillStyle = "#6b7280";
    ctx.font = "11px ui-monospace, monospace";
    ctx.fillText(track.id, 8, y + 14);
    // per-track color: a 3px swatch at the lane's left edge (the header color).
    const trackColor = TRACK_COLORS[ti % TRACK_COLORS.length];
    ctx.fillStyle = trackColor;
    ctx.fillRect(0, y, 3, laneH - 4);
    // clips
    for (const clip of track.clips) {
      const x = clip.at_frame * pxPerFrame;
      const wpx = clip.src_len * pxPerFrame;
      ctx.fillStyle = trackColor;
      ctx.fillRect(x, y + 4, Math.max(2, wpx), laneH - 12);
      ctx.strokeStyle = "rgba(255,255,255,0.35)";
      ctx.strokeRect(x, y + 4, Math.max(2, wpx), laneH - 12);
      // id label, only if the block is wide enough
      if (wpx > 24) {
        ctx.fillStyle = "#0b0d10";
        ctx.font = "10px ui-monospace, monospace";
        ctx.fillText(clip.id, x + 4, y + 16);
      }
    }
  });

  // ruler — a "nice" tick step whose labels are at least ~64 px apart
  ctx.fillStyle = "#8891a5";
  ctx.font = "10px ui-monospace, monospace";
  const nice = [0.1, 0.25, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300];
  const secPerPx = 1 / (pxPerFrame * RATE);
  const stepSec = nice.find((s) => s / secPerPx >= 64) ?? nice[nice.length - 1];
  for (let t = 0; t * RATE * pxPerFrame < w; t += stepSec) {
    const x = t * RATE * pxPerFrame;
    ctx.fillText(`${t >= 60 ? `${Math.floor(t / 60)}m` : ""}${(t % 60).toFixed(t < 1 ? 1 : 0)}s`, x + 2, 12);
  }

  // playhead — the live transport position (amber, per the design system)
  const px = bridgeState.position.frame * pxPerFrame;
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

/** A click sets the playhead (seek to the frame under the pointer). */
function seekFromClick(e: MouseEvent) {
  const c = canvas.value;
  if (!c) return;
  const rect = c.getBoundingClientRect();
  const x = e.clientX - rect.left;
  const frame = Math.max(0, Math.round(x / pxPerFrame));
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
      status.value = "no arrangement (add a `pool <dir>` + arrange add_clip) — showing demo";
    }
    bridgeState.status = status.value;
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

onMounted(() => {
  draw();
  window.addEventListener("resize", draw);
});
onUnmounted(() => window.removeEventListener("resize", draw));
</script>

<template>
  <div class="timeline">
    <div class="tl-bar">
      <span class="tl-status">{{ status }}</span>
      <input v-model="hostScript" class="tl-script" spellcheck="false" />
      <button class="btn" @click="loadFromHost">Run in host</button>
    </div>
    <canvas ref="canvas" class="tl-canvas" title="click to seek" @click="seekFromClick"></canvas>
  </div>
</template>

<style scoped>
.timeline { display: flex; flex-direction: column; height: 100%; min-height: 0; }
.tl-bar { display: flex; gap: 8px; align-items: center; padding: 8px; background: #17191f; border-bottom: 1px solid #22252c; }
.tl-status { font: 11px ui-monospace, monospace; color: #8b93a7; white-space: nowrap; max-width: 40%; overflow: hidden; text-overflow: ellipsis; }
.tl-script { flex: 1; font: 11px ui-monospace, monospace; color: #cfd6e4; background: #0e1013; border: 1px solid #262a32; border-radius: 4px; padding: 4px 6px; }
.tl-canvas { flex: 1; width: 100%; min-height: 0; display: block; cursor: text; }
</style>
