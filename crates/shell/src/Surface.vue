<script setup lang="ts">
// The shell: a minimal frame (top bar + Surface) that mounts a profile's
// ui-plugins into slots. It is intentionally thin — transport/undo chrome live
// here; the panels come from registered ui-plugins (src/plugins.ts).
//
// Transport and undo/redo are *chrome* for now: the Host-API bridge only drives
// a one-shot `run_host_script`, so play/stop/record and undo are placeholders
// that light up a local indicator. Wiring them to live engine commands is the
// next bridge step.
import { computed, ref } from "vue";
import { registry, soundArrangerProfile, type Profile, type SurfaceSlot } from "./plugins";
import { bridgeState } from "./bridge";

const props = defineProps<{ profile?: Profile }>();
const profile = computed(() => props.profile ?? soundArrangerProfile);

// The mixer is a pass, not the default state (docs/design/ui-plan.md §6).
const showMixer = ref(true);

// Demo transport indicator — replaced when live transport lands on the bridge.
const playing = ref(false);
function togglePlay() {
  playing.value = !playing.value;
  bridgeState.status = playing.value ? "transport playing (demo)" : "transport stopped (demo)";
}

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
        <button class="btn btn--icon" title="record (wiring pending)" disabled>⏺</button>
        <button class="btn btn--icon" title="stop" @click="playing = false; bridgeState.status = 'stopped'">■</button>
      </div>
      <span class="tempo"><b>{{ playing ? "120.0" : "—" }}</b> BPM</span>

      <div class="divider"></div>

      <div class="group" role="group" aria-label="edit">
        <button class="btn" title="undo (atomic undo/redo — wiring pending)" disabled>⟲</button>
        <button class="btn" title="redo" disabled>⟳</button>
        <button class="btn btn--icon" title="snap on/off" :class="{ 'is-active': true }">⌗</button>
        <button class="btn" title="zoom to fit">FIT</button>
      </div>

      <button class="btn" title="toggle the mixer panel" @click="showMixer = !showMixer">
        MIXER {{ showMixer ? "▣" : "▢" }}
      </button>

      <div class="spacer"></div>

      <span class="status" :title="bridgeState.status">{{ bridgeState.status }}</span>

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
