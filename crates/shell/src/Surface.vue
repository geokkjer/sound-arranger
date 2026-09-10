<script setup lang="ts">
// The shell: a minimal frame (top bar + Surface) that mounts a profile's
// ui-plugins into slots. It is intentionally thin — transport/undo chrome live
// here; the panels come from registered ui-plugins (src/plugins.ts).
//
// The transport and edit history are **live**: play/stop drive the host
// (host::live, on its own thread), the position + meters are polled from the
// bridge, and undo/redo replay the host's session log. Record remains chrome
// (it needs the device recording path).
import { computed, onMounted, onUnmounted, ref } from "vue";
import { registry, soundArrangerProfile, type Profile, type SurfaceSlot } from "./plugins";
import { bridgeState } from "./bridge";
import { formatTime, startTransportPoll, transportPlay, transportStop } from "./transport";
import { redoEdit, undoEdit } from "./editor";
import { RATE, fitTimeline, timelineView } from "./timelineView";

const props = defineProps<{ profile?: Profile }>();
const profile = computed(() => props.profile ?? soundArrangerProfile);

// The mixer is a pass, not the default state (docs/design/ui-plan.md §6).
const showMixer = ref(true);

const playing = computed(() => bridgeState.position.playing);
const timecode = computed(() => formatTime(bridgeState.position.seconds));
const bpm = computed(() => bridgeState.position.bpm);
// The timeline zoom, in pixels per second (frames × zoom).
const zoomPxPerSec = computed(() => Math.round(timelineView.zoom * RATE));

// The audio readout: the device the pump feeds (or why there is none).
const audioLabel = computed(() => {
  const a = bridgeState.audio;
  if (!a) return "♪ —";
  if (a.error) return "♪ ✕";
  const khz = (a.sample_rate / 1000).toFixed(1);
  return a.rate_mismatch ? `♪ ${khz}k ⚠` : `♪ ${khz}k`;
});
const audioTitle = computed(() => {
  const a = bridgeState.audio;
  if (!a) return "no audio output (silent host)";
  if (a.error) return `audio unavailable — ${a.error}`;
  const mismatch = a.rate_mismatch
    ? ` — DEVICE RATE MISMATCH (requested ${a.requested_rate} Hz): playback is off-speed`
    : "";
  return `${a.sample_rate} Hz · ${a.channels} ch — underruns ${a.underruns}, drops ${a.drops}${mismatch}`;
});
const audioClass = computed(() => ({
  "audio--warn": !!bridgeState.audio && !bridgeState.audio.error && bridgeState.audio.rate_mismatch,
  "audio--bad": !!bridgeState.audio?.error,
}));

function play() {
  transportPlay().catch((e) => (bridgeState.status = String(e)));
}
function stop() {
  transportStop().catch((e) => (bridgeState.status = String(e)));
}
function togglePlay() {
  if (playing.value) stop();
  else play();
}

function undo() {
  if (!bridgeState.canUndo) return;
  undoEdit().catch((e) => (bridgeState.status = String(e)));
}
function redo() {
  if (!bridgeState.canRedo) return;
  redoEdit().catch((e) => (bridgeState.status = String(e)));
}

// Ctrl/Cmd+Z (Shift to redo) anywhere except while typing in a text field, so
// the header buttons and the timeline share one history.
function onKey(e: KeyboardEvent) {
  if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
  if (e.key !== "z" && e.key !== "Z") return;
  const target = e.target as HTMLElement | null;
  if (target && (target.tagName === "TEXTAREA" || target.tagName === "INPUT")) return;
  e.preventDefault();
  if (e.shiftKey) redo();
  else undo();
}

// Poll the live host for the playhead + meters while the shell is mounted.
let stopPoll: (() => void) | null = null;
onMounted(() => {
  stopPoll = startTransportPoll();
  window.addEventListener("keydown", onKey);
});
onUnmounted(() => {
  stopPoll?.();
  window.removeEventListener("keydown", onKey);
});

// Resolve a profile slot to its registered view.
function slotView(slot: SurfaceSlot) {
  const entry = profile.value.slots.find((s) => s.slot === slot);
  return entry ? registry[entry.pluginId] : undefined;
}
</script>

<template>
  <div class="shell">
    <header class="bar">
      <div class="brand">
        <span class="glyph" aria-hidden="true"></span>
        <span>SOUND-<span class="muted">ARRANGER</span> <span class="muted">· {{ profile.name }}</span></span>
      </div>

      <div class="transport" role="group" aria-label="transport">
        <button class="btn btn--icon" :class="{ 'is-active': playing }" title="play" @click="togglePlay">▶</button>
        <button class="btn btn--icon" title="record (device recording not wired)" disabled>⏺</button>
        <button class="btn btn--icon" :class="{ 'is-active': !playing }" title="stop" @click="stop">■</button>
      </div>
      <span class="tempo"><b>{{ bpm.toFixed(1) }}</b> BPM</span>
      <span class="timecode">{{ timecode }}</span>
      <span class="audio" :class="audioClass" :title="audioTitle">{{ audioLabel }}</span>

      <div class="divider"></div>

      <div class="group" role="group" aria-label="edit">
        <button
          class="btn"
          :disabled="!bridgeState.canUndo"
          :title="bridgeState.canUndo ? 'undo the last edit (Ctrl+Z)' : 'nothing to undo'"
          @click="undo"
        >⟲</button>
        <button
          class="btn"
          :disabled="!bridgeState.canRedo"
          :title="bridgeState.canRedo ? 'redo the undone edit (Ctrl+Shift+Z)' : 'nothing to redo'"
          @click="redo"
        >⟳</button>
        <button class="btn btn--icon" title="snap on/off" :class="{ 'is-active': true }">⌗</button>
        <button class="btn" title="zoom to fit (the whole arrangement)" @click="fitTimeline">FIT</button>
        <button
          class="btn btn--icon"
          :class="{ 'is-active': timelineView.follow }"
          title="follow the playhead while playing"
          @click="timelineView.follow = !timelineView.follow"
        >⇥</button>
        <span class="zoom" title="timeline zoom">{{ zoomPxPerSec }} px/s</span>
      </div>

      <button class="btn" title="toggle the mixer panel" @click="showMixer = !showMixer">
        MIXER {{ showMixer ? "▣" : "▢" }}
      </button>

      <div class="spacer"></div>

      <span
        class="status"
        :class="{ 'status--error': bridgeState.lastError }"
        :title="bridgeState.lastError ?? bridgeState.status"
      >{{ bridgeState.lastError ?? bridgeState.status }}</span>

      <button class="btn" disabled>PROFILE ▾</button>
      <button class="btn btn--icon" title="settings">⚙</button>
    </header>

    <main class="surface">
      <aside v-if="slotView('left')" class="left">
        <component :is="slotView('left')!.component" />
      </aside>

      <div class="fill">
        <component v-if="slotView('fill')" :is="slotView('fill')!.component" />
      </div>

      <aside v-if="showMixer && slotView('right')" class="right">
        <component :is="slotView('right')!.component" />
      </aside>
    </main>

    <section v-if="slotView('bottom')" class="bottom">
      <component :is="slotView('bottom')!.component" />
    </section>
  </div>
</template>

<style scoped>
.shell {
  display: flex;
  flex-direction: column;
  height: 100%;
}

.bar {
  height: var(--bar-h);
  flex: 0 0 var(--bar-h);
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 0 14px;
  background: var(--panel);
  border-bottom: 1px solid var(--line);
}

.brand {
  display: flex;
  align-items: center;
  gap: 9px;
  letter-spacing: 0.14em;
  font-size: 12px;
  font-weight: 500;
}
.brand .glyph {
  width: 15px;
  height: 13px;
  position: relative;
}
.brand .glyph::before {
  content: "";
  position: absolute;
  inset: 0;
  border: 1.5px solid var(--accent);
  border-radius: 2px;
}
.brand .glyph::after {
  content: "";
  position: absolute;
  left: 2px;
  top: 3px;
  width: 2px;
  height: 5px;
  background: var(--accent);
}
.brand .muted {
  color: var(--fg-mute);
}

.transport,
.group {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.tempo {
  font: 11px ui-monospace, monospace;
  color: var(--fg-dim);
  white-space: nowrap;
}
.tempo b {
  color: var(--fg);
  font-weight: 500;
}

.timecode {
  font: 11px ui-monospace, monospace;
  color: var(--fg);
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
}

/* the audio readout: device rate, or why there is no output */
.audio {
  font: 11px ui-monospace, monospace;
  color: var(--fg-mute);
  white-space: nowrap;
}
.audio--warn {
  color: var(--warn);
}
.audio--bad {
  color: var(--record);
}

.zoom {
  font: 11px ui-monospace, monospace;
  color: var(--fg-mute);
  white-space: nowrap;
  font-variant-numeric: tabular-nums;
  min-width: 58px;
  text-align: right;
}

.divider {
  width: 1px;
  height: 18px;
  background: var(--line);
  margin: 0 2px;
}

.status {
  font: 11px ui-monospace, monospace;
  color: var(--fg-mute);
  white-space: nowrap;
  max-width: 30%;
  overflow: hidden;
  text-overflow: ellipsis;
}
.status--error {
  color: var(--record);
}

.spacer {
  flex: 1;
}

.surface {
  flex: 1;
  display: flex;
  align-items: stretch;
  min-height: 0;
  background: var(--surface);
}
.fill {
  flex: 1;
  min-width: 0;
  min-height: 0;
  display: flex;
}
.left,
.right {
  flex-shrink: 0;
  min-height: 0;
}
.left {
  width: 240px;
  border-right: 1px solid var(--line);
}
.right {
  width: 208px;
  border-left: 1px solid var(--line);
}
.bottom {
  flex: 0 0 auto;
  max-height: 30%;
  min-height: 30px;
  display: flex;
  flex-direction: column;
}
</style>
