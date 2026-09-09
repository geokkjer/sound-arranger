<script setup lang="ts">
// The clip-arranger profile surface: a timeline canvas ui-plugin (hero/fill) and
// a mixer panel (right), both backed by the Host-API bridge. The bridge's meter
// snapshot is lifted from the canvas and fed to the mixer. Source/pool and
// interactive editing are the next ui-plugins.
import { ref } from "vue";
import TimelineCanvas from "./TimelineCanvas.vue";
import MixerPanel from "./MixerPanel.vue";

interface MixerMeters { channels: number[]; master: number; }
const meters = ref<MixerMeters | null>(null);
</script>

<template>
  <div class="shell">
    <header class="bar">
      <div class="brand">
        <span class="glyph" aria-hidden="true"></span>
        <span>SOUND-<span class="muted">ARRANGER</span> <span class="muted">· clip arranger</span></span>
      </div>
      <div class="spacer"></div>
      <button class="btn" disabled>PROFILE ▾</button>
      <button class="btn">⚙</button>
    </header>

    <main class="surface">
      <div class="fill">
        <TimelineCanvas @meters="meters = $event" />
      </div>
      <aside class="right">
        <MixerPanel :meters="meters" />
      </aside>
    </main>
  </div>
</template>
