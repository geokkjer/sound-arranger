<script setup lang="ts">
// The source-pool ui-plugin, per the revised docs/design/ui-plan.md §6: two
// contexts — **Project** (this session's recorded/copied material, i.e. the
// media pool) and **Library** (a referenced folder, default ~/Music/Samples).
// The project context lists the bridge's pool sources with search; the library
// context is a placeholder until the folder watcher + drag-in land. Dragging a
// source onto a track is the next interactivity step.
import { computed, ref } from "vue";
import { bridgeState, type PoolSource } from "./bridge";

type PoolTab = "project" | "library";

const tab = ref<PoolTab>("project");
const query = ref("");

const sources = computed<PoolSource[]>(() => {
  const q = query.value.trim().toLowerCase();
  const all = bridgeState.sources;
  return q ? all.filter((s) => s.id.toLowerCase().includes(q)) : all;
});
</script>

<template>
  <div class="pool">
    <div class="pool-title">SOURCE POOL</div>

    <div class="pool-tabs">
      <button
        class="pool-tab"
        :class="{ 'is-active': tab === 'project' }"
        @click="tab = 'project'"
      >PROJECT</button>
      <button
        class="pool-tab"
        :class="{ 'is-active': tab === 'library' }"
        @click="tab = 'library'"
      >LIBRARY</button>
    </div>

    <input v-model="query" class="pool-search" placeholder="search (⌘F)" spellcheck="false" />

    <template v-if="tab === 'project'">
      <div v-if="sources.length === 0" class="pool-empty">
        {{ bridgeState.sources.length === 0
          ? "No pool set. Add a `pool <dir>` line to load sources."
          : "No sources match the filter." }}
      </div>
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
    </template>

    <div v-else class="pool-empty pool-empty--library">
      Library — a referenced folder (default <code>~/Music/Samples</code>). The folder watcher and
      drag-to-timeline are the next step; promote a clip here to reuse it across projects.
    </div>
  </div>
</template>

<style scoped>
.pool {
  display: flex;
  flex-direction: column;
  height: 100%;
  padding: 10px;
  box-sizing: border-box;
  gap: 8px;
}
.pool-title {
  font: 11px ui-monospace, monospace;
  color: #8b93a7;
}
.pool-tabs {
  display: inline-flex;
  gap: 2px;
}
.pool-tab {
  height: 20px;
  padding: 0 8px;
  font: 10px ui-monospace, monospace;
  letter-spacing: 0.1em;
  color: #8891a5;
  background: transparent;
  border: 1px solid #262a32;
  border-radius: 3px;
  cursor: pointer;
}
.pool-tab:hover {
  color: #cfd6e4;
  background: #17191f;
}
.pool-tab.is-active {
  color: #6b8afd;
  border-color: #6b8afd;
}
.pool-search {
  font: 11px ui-monospace, monospace;
  color: #cfd6e4;
  background: #0e1013;
  border: 1px solid #262a32;
  border-radius: 4px;
  padding: 4px 8px;
}
.pool-search::placeholder {
  color: #6b7280;
}
.pool-empty {
  font: 11px ui-monospace, monospace;
  color: #6b7280;
}
.pool-empty code {
  color: #8891a5;
}
.pool-empty--library {
  padding: 8px 10px;
  border: 1px dashed #262a32;
  border-radius: 4px;
  line-height: 1.6;
}
.pool-list {
  list-style: none;
  margin: 0;
  padding: 0;
  overflow: auto;
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.pool-item {
  border: 1px solid #262a32;
  border-radius: 4px;
  padding: 6px 8px;
  background: #14161a;
}
.pool-id {
  font: 12px ui-monospace, monospace;
  color: #cfd6e4;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pool-id.pool-bad {
  color: #f59e0b;
}
.pool-meta {
  display: flex;
  gap: 8px;
  margin-top: 3px;
  font: 10px ui-monospace, monospace;
  color: #8b93a7;
}
.pool-flag {
  color: #f59e0b;
}
</style>
