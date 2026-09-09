# Agent Note: lean ui-plugin runtime — registry, profile, slotted Surface, and the shared bridge store

Status: implemented

## Problem

The shell (`crates/shell/src/App.vue`) was a hardcoded clip-arranger profile: it imported
`SourcePool`, `TimelineCanvas`, and `MixerPanel` directly and laid them out by hand, lifting
meters/sources from the canvas via `defineEmits` and prop-drilling them to the other two panels.
The [ui-shell note](../../proposed/architecture/2026-08-25-ui-shell-profiles-and-views.md) and [`docs/design/ui-plan.md`
§1-2](../../../../docs/design/ui-plan.md) specify a shell + registry + profile + Surface-slot model, and
the revised design (2026-09-09) adds a **bottom Detail View** slot and a **toggleable mixer**.
None of that existed: there was no registry, no profile-as-data, no slot abstraction, no bottom
slot, and the cross-view bridge state was threaded by hand.

## Decision

Build a *lean* v1 of the runtime — the parts needed to make the UI examinable — not the full
Command/Overlay/State/Bridge/Preference surface the note describes:

1. **Registry + profile** (`src/plugins.ts`): a `UiPluginView` type (`id · name · slot ·
   component`), a `Registry` mapping plugin ids to views, `SurfaceSlot = fill|left|right|bottom`,
   and a `Profile` (id · name · ordered `slots: {pluginId, slot}[]`). The default
   `soundArrangerProfile` places source-pool / timeline / mixer / detail into left / fill / right
   / bottom. `Profile` is data — swapping it swaps the instrument.
2. **Shared bridge store** (`src/bridge.ts`): a reactive `bridgeState` (status, meters, sources,
   arrangement) that mirrors the `ScriptOutcome` serde shape. The timeline canvas is the only
   writer (running the host script lifts the result here); source-pool and mixer read it. This
   removes prop-drilling through the slot renderer and is the shallow stand-in for the note's
   `State` + `Bridge` contribution types. `meterFill` (dB → 0..1) moved here for reuse.
3. **Slotted Surface + top bar** (`src/Surface.vue`): renders the profile's views into the four
   slots (`<component :is>` in the matching geometry), with the top bar hosting brand, transport,
   tempo, undo/redo, snap, a mixer toggle, status, and the profile switcher. Transport/undo/record
   remain **chrome placeholders** — the Host-API bridge only drives a one-shot `run_host_script`,
   so there is no live transport yet. `App.vue` is now just `<Surface :profile="…" />`.
4. **Bottom Detail View** (`src/DetailView.vue`): a contextual slot that **collapses** to its
   header when nothing is focused, with tabs that sketch the three contexts (region / chain /
   source). The bodies are placeholders until selection is wired.
5. The mixer is **toggleable** via the top bar (it's a pass, not the default state).

The three existing panels were refactored onto the store: `TimelineCanvas` writes
`bridgeState`, `SourcePool` reads `bridgeState.sources`, `MixerPanel` reads `bridgeState.meters`.

## Alternatives considered

- **Prop-drill through the slot renderer** (keep `defineEmits` + props, bind by plugin id).
  Rejected: a generic `<component :is>` slot placement has no clean prop channel; a shared
  reactive store keeps the slot renderer free of per-view wiring and matches the note's `State`
  contribution. Chosen.
- **Build the full Command/Overlay/State/Bridge/Preference runtime now.** Rejected as scope creep
  for the milestone (the note itself flags plugin-for-everything as out of scope); ship the
  View + slot + profile core first.
- **A fully generic quad-slot renderer** (e.g. a slot registry that fills any 2D grid).
  Rejected: the profile has known, fixed geometry (left / fill / right / bottom with
  fill-as-hero); hardcoding the geometry is simpler and keeps WebKit perf predictable.
- **Wire live transport/undo to the bridge now.** Rejected: the bridge is one-shot
  (`run_host_script`); live transport needs a growing bridge (`execute()` per command, position
  events). Left as a placeholder — the chrome is correct, the wiring is a later step.

## Consequences

- The shell is now **data-driven**: the profile list decides which view occupies which slot.
- Cross-view bridge state lives in one place; panels no longer need re-wiring when slots change.
- The revised profile's **bottom Detail View** and **toggleable mixer** are represented.
- Transport/undo/record are on-screen but not wired (documented in `Surface.vue`); the host-script
  runner stays in the timeline canvas as scaffolding until the bridge grows live commands.
- Not yet: Command/Overlay/Bridge/Preference contributions, live transport, selection → Detail View
  focus, per-track colors (next commit), resizable panels, drag-to-timeline.
- Build stays green (`vue-tsc --noEmit` + `vite build`), 31 modules.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-09.
