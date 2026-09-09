<script setup lang="ts">
// The clip-arranger profile surface: source pool (left), timeline canvas
// (hero/fill), mixer panel (right) — three ui-plugins backed by the Host-API
// bridge. The bridge's meter + pool-listing snapshots are lifted from the
// canvas. Interactive editing (drag / razor-split) is the next ui-plugin step.
import { ref } from "vue";
import TimelineCanvas from "./TimelineCanvas.vue";
import MixerPanel from "./MixerPanel.vue";
import SourcePool from "./SourcePool.vue";

interface MixerMeters { channels: number[]; master: number; }
interface PoolSource { id: string; frames: number; sample_rate: number; peaks_missing: boolean; finalized: boolean; }
const meters = ref<MixerMeters | null>(null);
const sources = ref<PoolSource[]>([]);
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
      <aside class="left">
        <SourcePool :sources="sources" />
      </aside>
      <div class="fill">
        <TimelineCanvas @meters="meters = $event" @sources="sources = $event ?? []" />
      </div>
      <aside class="right">
        <MixerPanel :meters="meters" />
      </aside>
    </main>
  </div>
</template>
