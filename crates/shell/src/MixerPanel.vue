<script setup lang="ts">
// The mixer panel ui-plugin: strips per channel (gain/mute/solo/pan are set via
// the host script for now) and a live meter bank drawn from the bridge's
// ScriptOutcome.mixer_meters (per-channel post-gain/pre-mute peaks + the master).
// Meter level is clamped to [-60 dB, 0] and rendered as a simple bar.
import { computed, type PropType } from "vue";

interface MixerMeters {
  channels: number[];
  master: number;
}

const props = defineProps({
  meters: { type: Object as PropType<MixerMeters | null>, default: null },
});

const level = (p: number) => Math.min(1, Math.max(0, (20 * Math.log10(Math.max(p, 1e-9)) + 60) / 60));
const strips = computed(() => {
  const ch = props.meters?.channels ?? [];
  return Array.from({ length: Math.max(2, ch.length) }, (_, i) => i);
});
</script>

<template>
  <div class="mixer">
    <div class="mx-title">MIXER</div>
    <div class="mx-strips">
      <div v-for="i in strips" :key="i" class="mx-strip">
        <div class="mx-num">ch{{ i }}</div>
        <div class="mx-meter">
          <div
            class="mx-bar"
            :style="{ height: `${(level(meters?.channels[i] ?? 0) * 100).toFixed(1)}%` }"
          ></div>
        </div>
      </div>
    </div>
    <div class="mx-master">
      <div class="mx-num">MASTER</div>
      <div class="mx-meter">
        <div
          class="mx-bar mx-bar--master"
          :style="{ height: `${(level(meters?.master ?? 0) * 100).toFixed(1)}%` }"
        ></div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.mixer { display: flex; flex-direction: column; gap: 8px; padding: 10px; height: 100%; box-sizing: border-box; }
.mx-title { font: 11px ui-monospace, monospace; color: #8b93a7; }
.mx-strips { display: flex; flex: 1; gap: 6px; align-items: stretch; min-height: 0; }
.mx-strip { flex: 1; display: flex; flex-direction: column; gap: 4px; min-width: 24px; }
.mx-num { font: 10px ui-monospace, monospace; color: #8891a5; text-align: center; }
.mx-meter { flex: 1; position: relative; background: #0e1013; border: 1px solid #262a32; border-radius: 3px; overflow: hidden; min-height: 0; }
.mx-bar { position: absolute; left: 0; right: 0; bottom: 0; background: linear-gradient(180deg, #38bdf8, #2563eb); }
.mx-bar--master { background: linear-gradient(180deg, #fbbf24, #b45309); }
.mx-master { display: flex; flex-direction: column; gap: 4px; }
</style>
