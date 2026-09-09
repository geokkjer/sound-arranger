<script setup lang="ts">
// The bottom Detail View — contextual on what has focus, per the revised
// docs/design/ui-plan.md §6. When nothing is focused it collapses to its header
// (the timeline stays the hero); focusing a clip / track / library item would
// flip it to a region editor, effect-chain editor, or source editor.
//
// Selection isn't wired yet, so the body is a placeholder: each context tab
// shows what it *will* edit, and the panel starts collapsed. The collapsed
// state is the real behavior worth seeing; the bodies sketch the three contexts.
import { ref } from "vue";

type FocusContext = "region" | "chain" | "source";

const focused = ref<FocusContext | null>(null);
const expanded = ref(false);

const CONTEXTS: Array<{ id: FocusContext; label: string; hint: string }> = [
  {
    id: "region",
    label: "REGION",
    hint: "gain · fades · fill rule (loop/trim) · loop points — the edit for a selected clip/region.",
  },
  {
    id: "chain",
    label: "CHAIN",
    hint: "the track effect chain (fundsp nodes, order, in/out, sends) — the future CDP offline chain lives here too.",
  },
  {
    id: "source",
    label: "SOURCE",
    hint: "loop points, one-shot vs loop mode, gain, audition — for a library/loop item being previewed.",
  },
];

function toggleTab(id: FocusContext) {
  if (focused.value === id && expanded.value) {
    expanded.value = false;
  } else {
    focused.value = id;
    expanded.value = true;
  }
}
</script>

<template>
  <div class="detail" :class="{ 'detail--empty': !focused }">
    <div class="detail-tabs">
      <button
        v-for="c in CONTEXTS"
        :key="c.id"
        class="detail-tab"
        :class="{ 'detail-tab--active': focused === c.id && expanded }"
        :title="c.hint"
        @click="toggleTab(c.id)"
      >
        {{ c.label }}
      </button>
      <div class="spacer"></div>
      <span v-if="!focused" class="detail-hint">nothing focused — select a clip, track, or library item</span>
    </div>
    <div v-if="expanded && focused" class="detail-body">
      <div class="detail-placeholder">
        {{ CONTEXTS.find((c) => c.id === focused)?.hint }}
      </div>
    </div>
  </div>
</template>

<style scoped>
.detail {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--surface);
  border-top: 1px solid var(--line);
  min-height: 0;
}
.detail-tabs {
  display: flex;
  align-items: center;
  gap: 6px;
  height: 30px;
  padding: 0 8px;
  background: var(--panel);
  border-bottom: 1px solid var(--line);
}
.detail-tab {
  height: 20px;
  padding: 0 8px;
  font: 10px ui-monospace, monospace;
  letter-spacing: 0.1em;
  color: var(--fg-dim);
  background: transparent;
  border: 1px solid transparent;
  border-radius: var(--radius-sm);
  cursor: pointer;
}
.detail-tab:hover {
  color: var(--fg);
  background: var(--hover);
}
.detail-tab--active {
  color: var(--accent);
  border-color: var(--accent);
}
.detail-hint {
  font: 11px ui-monospace, monospace;
  color: var(--fg-mute);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.detail-body {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 10px 12px;
}
.detail-placeholder {
  font: 12px ui-monospace, monospace;
  color: var(--fg-dim);
  max-width: 68ch;
}
</style>
