<script setup lang="ts">
// The timeline canvas — the first real ui-plugin of the clip-arranger profile.
// It draws the arrangement value (tracks → clips) as blocks on a frame-scaled
// timeline, and wires the Host-API bridge (Tauri `run_host_script`) so a real
// arrangement from the engine is rendered when the user points it at a pool.
//
// The demo arrangement below lets the canvas show something without a media
// pool; the "Load from host" path drives the engine (it needs `mount mixer` +
// a `pool <dir>` with float-WAV sources in the script).

import { invoke } from "@tauri-apps/api/core";
import { onMounted, onUnmounted, ref } from "vue";

// The arrangement value as serialized by `media::Timeline` (serde, snake_case).
interface Clip {
  id: string;
  source: string;
  src_start: number;
  src_len: number;
  at_frame: number;
  fade_in: number;
  fade_out: number;
  gain: number;
  loop_len: number | null;
}
interface Track {
  id: string;
  clips: Clip[];
}
interface Timeline {
  tracks: Track[];
}

const hostScript = ref(
  "host v1\nmount mixer channels=4 @0\narrange add_track t0 @0\narrange add_track t1 @0",
);
const status = ref("drawing demo arrangement");

// The bridge's meter snapshot + pool listing, emitted to the shell so the mixer
// and source-pool panels can draw without a second round-trip.
interface MixerMeters { channels: number[]; master: number; }
interface PoolSource {
  id: string; wav: string; peaks: string; frames: number; sample_rate: number;
  peaks_missing: boolean; finalized: boolean;
}
const emit = defineEmits<{
  (e: "meters", meters: MixerMeters | null): void;
  (e: "sources", sources: PoolSource[] | null): void;
}>();

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
let pxPerFrame = 0.01; // 100 px per second at 48 kHz
let lastTime = 0;

const clamp = (n: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, n));

function draw(tl: Timeline) {
  const c = canvas.value;
  if (!c) return;
  const ctx = c.getContext("2d");
  if (!ctx) return;
  const dpr = window.devicePixelRatio || 1;
  const w = c.clientWidth, h = c.clientHeight;
  if (c.width !== w * dpr || c.height !== h * dpr) {
    c.width = w * dpr;
    c.height = h * dpr;
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);

  // background
  ctx.fillStyle = "#14161a";
  ctx.fillRect(0, 0, w, h);

  const laneH = Math.max(48, (h - 20) / Math.max(1, tl.tracks.length));
  tl.tracks.forEach((track, ti) => {
    const y = 20 + ti * laneH;
    // lane background + label
    ctx.fillStyle = "#1d2026";
    ctx.fillRect(0, y, w, laneH - 4);
    ctx.fillStyle = "#6b7280";
    ctx.font = "11px ui-monospace, monospace";
    ctx.fillText(track.id, 8, y + 14);
    // clips
    for (const clip of track.clips) {
      const x = clip.at_frame * pxPerFrame;
      const wpx = clip.src_len * pxPerFrame;
      const hue = clip.source === "s2" ? 210 : clip.source === "s3" ? 280 : 170;
      ctx.fillStyle = `hsla(${hue}, 60%, 55%, 0.85)`;
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
  // ruler
  ctx.fillStyle = "#8891a5";
  ctx.font = "10px ui-monospace, monospace";
  const step = 48000 * Math.max(0.25, Math.round((1 / (pxPerFrame * 48000)))); // nice clamp
  for (let f = 0; f * pxPerFrame < w; f += step) {
    ctx.fillText(`${(f / 48000).toFixed(1)}s`, f * pxPerFrame + 2, 12);
  }
}

function resize() {
  draw(demo);
}

async function loadFromHost() {
  try {
    status.value = "running host script…";
    const outcome = await invoke<{ arrangement: Timeline | null; mixer_meters: MixerMeters | null; pool_sources: PoolSource[] | null }>("run_host_script", {
      scriptText: hostScript.value,
    });
    emit("meters", outcome.mixer_meters);
    emit("sources", outcome.pool_sources);
    if (outcome.arrangement && outcome.arrangement.tracks.length > 0) {
      status.value = `loaded ${outcome.arrangement.tracks.length} track(s) from the engine`;
      draw(outcome.arrangement);
    } else {
      status.value = "host produced no arrangement (add a `pool <dir>` + arrange add_clip) — showing demo";
      draw(demo);
    }
  } catch (e) {
    status.value = String(e);
    emit("meters", null);
    emit("sources", null);
    draw(demo);
  }
}

onMounted(() => {
  resize();
  window.addEventListener("resize", resize);
  lastTime = performance.now();
});
onUnmounted(() => window.removeEventListener("resize", resize));
</script>

<template>
  <div class="timeline">
    <div class="tl-bar">
      <span class="tl-status">{{ status }}</span>
      <input v-model="hostScript" class="tl-script" spellcheck="false" />
      <button class="btn" @click="loadFromHost">Run in host</button>
    </div>
    <canvas ref="canvas" class="tl-canvas"></canvas>
  </div>
</template>

<style scoped>
.timeline { display: flex; flex-direction: column; height: 100%; min-height: 0; }
.tl-bar { display: flex; gap: 8px; align-items: center; padding: 8px; background: #17191f; border-bottom: 1px solid #22252c; }
.tl-status { font: 11px ui-monospace, monospace; color: #8b93a7; white-space: nowrap; max-width: 40%; overflow: hidden; text-overflow: ellipsis; }
.tl-script { flex: 1; font: 11px ui-monospace, monospace; color: #cfd6e4; background: #0e1013; border: 1px solid #262a32; border-radius: 4px; padding: 4px 6px; }
.tl-canvas { flex: 1; width: 100%; min-height: 0; display: block; }
</style>
