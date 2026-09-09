import type { Component } from "vue";
import SourcePool from "./SourcePool.vue";
import TimelineCanvas from "./TimelineCanvas.vue";
import MixerPanel from "./MixerPanel.vue";
import DetailView from "./DetailView.vue";

// The ui-plugin runtime, per the ui-shell note and docs/design/ui-plan.md §1-2.
// A `ui-plugin` describes a `View` (a panel that occupies a Surface slot) for
// the clip-arranger profile. Command / Overlay / State / Bridge / Preference are
// declared in the plan but not yet wired for v1 — the shared bridge state
// (`src/bridge.ts`) is the shallow stand-in for `State` + `Bridge`.

export type SurfaceSlot = "fill" | "left" | "right" | "bottom";

export interface UiPluginView {
  /** stable id, e.g. "source-pool" — how a profile references it. */
  readonly id: string;
  /** display name shown in the top bar / profile switcher. */
  readonly name: string;
  /** the slot this view occupies by default. */
  readonly slot: SurfaceSlot;
  /** the component rendered in the slot. */
  readonly component: Component;
}

export interface Profile {
  readonly id: string;
  readonly name: string;
  /** ordered: which plugin renders in which slot. A slot may be left empty. */
  readonly slots: ReadonlyArray<{ pluginId: string; slot: SurfaceSlot }>;
}

/** The installed ui-plugin registry (declarations are usually the manifest). */
export const registry: Record<string, UiPluginView> = {
  "source-pool": {
    id: "source-pool",
    name: "Source Pool",
    slot: "left",
    component: SourcePool,
  },
  timeline: { id: "timeline", name: "Timeline", slot: "fill", component: TimelineCanvas },
  mixer: { id: "mixer", name: "Mixer", slot: "right", component: MixerPanel },
  detail: { id: "detail", name: "Detail View", slot: "bottom", component: DetailView },
};

/** The default profile: source pool · timeline (hero) · mixer · detail view. */
export const soundArrangerProfile: Profile = {
  id: "sound-arranger",
  name: "clip arranger",
  slots: [
    { pluginId: "source-pool", slot: "left" },
    { pluginId: "timeline", slot: "fill" },
    { pluginId: "mixer", slot: "right" },
    { pluginId: "detail", slot: "bottom" },
  ],
};

/** Resolve a profile's slot entry to its registered view. */
export function viewFor(pluginId: string): UiPluginView | undefined {
  return registry[pluginId];
}
