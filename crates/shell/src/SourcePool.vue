<script setup lang="ts">
// The source-pool ui-plugin: lists the media pool's take/loop sources (id, frame
// count, sample rate, peaks state) so you can see what's available to place on
// the timeline. Dragging a source onto a track is the next interactivity step.
import { type PropType } from "vue";

interface PoolSource {
  id: string; frames: number; sample_rate: number; peaks_missing: boolean; finalized: boolean;
}
defineProps({
  sources: { type: Array as PropType<PoolSource[]>, default: () => [] },
});
</script>

<template>
  <div class="pool">
    <div class="pool-title">SOURCE POOL</div>
    <div v-if="sources.length === 0" class="pool-empty">No pool set. Add a <code>pool &lt;dir&gt;</code> line to load sources.</div>
    <ul class="pool-list">
      <li v-for="s in sources" :key="s.id" class="pool-item">
        <div class="pool-id" :class="{ 'pool-bad': s.peaks_missing }">{{ s.id }}</div>
        <div class="pool-meta">
          <span>{{ (s.frames / (s.sample_rate || 1)).toFixed(1) }}s</span>
          <span>{{ (s.sample_rate / 1000).toFixed(1) }}kHz</span>
          <span v-if="s.peaks_missing" class="pool-flag" title="missing .peaks sidecar">~peaks</span>
          <span v-if="!s.finalized" class="pool-flag" title="un-finalized (crashed) take — needs recovery">unfinalized</span>
        </div>
      </li>
    </ul>
  </div>
</template>

<style scoped>
.pool { display: flex; flex-direction: column; height: 100%; padding: 10px; box-sizing: border-box; }
.pool-title { font: 11px ui-monospace, monospace; color: #8b93a7; }
.pool-empty { font: 11px ui-monospace, monospace; color: #6b7280; margin-top: 8px; }
.pool-list { list-style: none; margin: 8px 0 0; padding: 0; overflow: auto; display: flex; flex-direction: column; gap: 6px; }
.pool-item { border: 1px solid #262a32; border-radius: 4px; padding: 6px 8px; background: #14161a; }
.pool-id { font: 12px ui-monospace, monospace; color: #cfd6e4; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pool-id.pool-bad { color: #f59e0b; }
.pool-meta { display: flex; gap: 8px; margin-top: 3px; font: 10px ui-monospace, monospace; color: #8b93a7; }
.pool-flag { color: #f59e0b; }
</style>
