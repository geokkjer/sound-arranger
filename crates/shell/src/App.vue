<script setup lang="ts">
// The shell entry: mount the default profile into the Surface. The shell is a
// slotted container; the profile (which ui-plugins render in which slots) lives
// in src/plugins.ts. See docs/design/ui-plan.md §1-2 and the ui-shell note.
import Surface from "./Surface.vue";
import { soundArrangerProfile } from "./plugins";
</script>

<template>
  <Surface :profile="soundArrangerProfile" />
</template>
