<script setup lang="ts">
// The clip-arranger profile surface: a timeline canvas ui-plugin in the hero
// slot, backed by the Host-API bridge (run_host_script). Source pool + mixer
// panels are the next ui-plugins.
import TimelineCanvas from "./TimelineCanvas.vue";
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
      <TimelineCanvas />
    </main>
  </div>
</template>
