<script setup lang="ts">
// Core shell placeholder. A profile (a set of ui-plugins) will populate the
// surface; until one loads the shell stays empty. See docs/design/ui-plan.md.
</script>

<template>
  <div class="shell">
    <header class="bar">
      <div class="brand">
        <span class="glyph" aria-hidden="true"></span>
        <span>SOUND-<span class="muted">ARRANGER</span> <span class="muted">· core</span></span>
      </div>
      <div class="spacer"></div>
      <button class="btn" disabled>PROFILE ▾</button>
      <button class="btn">⚙</button>
    </header>

    <main class="surface">
      <div class="empty">
        <div class="ring" aria-hidden="true"></div>
        <h1>Load a profile</h1>
        <p>Profiles assemble plugins. The surface stays empty until one loads.</p>
        <div>
          <kbd>⌘</kbd><kbd>k</kbd> open profile &nbsp;·&nbsp;
          <kbd>⌘</kbd><kbd>shift</kbd><kbd>p</kbd> command palette
        </div>
      </div>
    </main>
  </div>
</template>
