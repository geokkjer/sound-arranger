# sound-arranger — Desktop UI Plan (the Vue side of the Host API)

> **Status: proposed (design draft).** No UI code exists yet (`crates/host` is the reference; the Tauri shell is the next step per the [UI-as-plugin note](../../.agents/notes/implemented/architecture/2026-08-18-ui-as-plugin-host-api-and-headless-reference.md)). This plan is the concrete design of the **Vue host-side plugin runtime** that note's "Next" calls for: the shell + profile + view layer that turns the Host API contract into a usable instrument.

This document owns the *shape* and the *tradeoffs of the UI*. The visual/token contract lives in [`design-system.md`](design-system.md); the wireframes are in [`mocks/`](mocks/). The reference webview is **WebKitGTK 2.52.6** (`webkit2gtk-4.1`), i.e. Linux and the primary target.

> **Revised 2026-09-09** against the Ableton Live prior-art review (the Arrangement view, not the Session grid). The source model (Project vs Library), the unified clip-as-loop model, and the relaxed CDP seam are owned by the [UI revision note](../../.agents/notes/proposed/architecture/2026-09-09-ui-revision-project-library-and-clip-model.md); the observations they come from are in the [prior-art study](../../research/architecture/2026-09-09-ableton-prior-art-ui.md).

---

## 0. The single governing constraint — the renderer

Tauri v2 on Linux renders through **WebKitGTK**, not Chromium. It is slower at compositing and re-layout than Chromium, and a handful of features are disproportionately expensive:

| Feature | Verdict |
|---|---|
| `backdrop-filter` / `filter: blur` | **Avoid.** The single worst compositing offender for panes. |
| Large `box-shadow` | **Avoid.** Prefer 1px hairline borders. |
| `will-change` on many nodes | **Avoid** (memory spikes). Use on one or two moving layers max. |
| Deep flex/grid re-layout during transport ticks | **Avoid** — repaint the clock on a small canvas/text, never relayout the bar. |
| DOM-per-clip / DOM-per-waveform-column | **Avoid** — that's what the canvas timeline is for. |
| Modern CSS (`oklch`, `color-mix()`, `@property`) | **Supported.** WebKitGTK 2.52.6 is well past the bar; use it in the theme layer. |

**The engineer-minimalist aesthetic is not a taste preference here — it is the correct performance decision.** Fewer composited layers, flat surfaces, thin chrome, and canvases for anything that moves at audio/drag rate. This is a rare case where "looks quieter" and "runs faster" are the same change.

---

## 1. Architecture (mirrors the Host API seam)

The Host API note already fixed the two connection modes; the UI is the *host_ side. Do **not** invent a single plugin interface that covers both render and audio — that is the leaky-abstraction trap. Keep two channels and a thin declared bridge.

### Core UI (the shell, always present)

A minimal frame. Never more than this:

- **Top bar** (`~44px`, solid, flat): brand identity · clock+transport scaffold · profile switcher · a few global utilities (grid, settings). Cheap to composite; never scrolls; never blurs.
- **Surface / Workspace** (`fill`): a **slotted container** that hosts profile views. Empty until a profile loads.
- **Runtime**: declarative loader, plugin registry, command dispatch, atomic undo/redo, theming, clock/transport scaffold.
- **Shell/chrome primitives** (components, not plugins): button, tooltip, menu, popover, combobox, scroll-area, tabs, divider.

### The plugin model (Vue side)

- **`engine-plugin`** — audio: DSP / graph node / param set. Connects **in-process** via the `Plugin` trait. (You largely have this; the Host API never touches the render path.)
- **`ui-plugin`** — contributes to the workspace. Connects **through the Host API** (control-rate, cross-boundary). It can contribute:
  - **`View`** — a panel that occupies a Surface slot.
  - **`Command`** — a top-bar action + keyboard binding + undo recipe.
  - **`Overlay`** — a transient layer (selection marquee, splitter, hint, inspector).
  - **`State`** — its slice of app state.
  - **`Bridge`** — a *declared* mapping to engine plugins/params (not a shared interface).
  - **`Preference`** — a persisted setting surfaced in settings.
- **`profile`** — an ordered list of plugins + a **`Layout`** (which views go in which slots), **`Keymap`**, **`DefaultState`**, **`Wiring`**. **A profile is itself a plugin**, so profiles compose and can be shared/inherited. Switching profiles = switching instruments.

### Registration / lifecycle

Declarative manifest (`open-design.json`-style, but this is our own schema) → registry resolves → runtime mounts the profile → views are placed into Surface slots → wired. Idempotent and hot-swappable: adding/removing a plugin reflows the Surface without a reload. This mirrors the engine's plugin discipline so the UI stays interchangeable.

### Why two channels (and not one)

If a single interface had to be both a graph node and a DOM view, a headless batch render (no UI) or a viewless DSP node (no audio) would be forced to satisfy dead contracts. Two channels + a `Bridge` keeps the boundary honest and matches the Host API note's existing structure exactly.

---

## 2. Surface / slot model

- Slots: `fill` · `left` · `right` · `bottom`. Views declare the slots they can occupy and a default slot.
- `fill` (the timeline) is the hero; the default profile puts it center and full width.
- Reflow: resizing the window moves flex space between slots. `left`/`right`/`bottom` panels keep their width (user-settable in a range); `fill` absorbs the remainder. So wider windows yield **more timeline**, not more empty panels.
- Only one primary `fill` view per profile at a time. Secondary content is of *two* kinds: a **persistent Detail View** (`bottom`) that is contextual on focus and collapses when nothing is focused, and transient `Overlay`s (marquee, splitter, hint). The Detail View is a real slot, not an overlay.

---

## 3. Component & rendering strategy — Hybrid

Locked decision. Three tiers, each with a clear rationale:

1. **Headless base primitives → `reka-ui`.** Behavior + a11y only, no styling weight. You'd otherwise re-implement focus traps and ARIA for nothing. Safe on WebKit.
2. **Audio-native, high-frequency widgets → custom Canvas 2D.** Timeline + clips, waveforms (peaks), level meters, gain/knob curves, transport clock, playhead. Canvas is fast and predictable; DOM-per-column is a dead end. (Timeline canvas was already locked; extend the same rule to meters/knobs.)
3. **Thin CSS for the chrome** — scoped CSS (or CSS Modules) for the perf-critical chrome; **Tailwind v4 for layout utility only, not the hot path.** On WebKit, fewer utility nodes and no utility-driven re-layout is strictly better. Use modern CSS (`oklch`, `color-mix()`, `@property`) in the theme layer; keep the render loop plain.

### Timeline viewport & zoom model

The timeline is a **fixed time-space viewed through a scrolling window**, so zoom is a *mapping change*, not a relayout of per-clip DOM. This is the model to implement in the canvas:

- **State:** `zoom` (pixels-per-second), `t0` (view start time, i.e. the scroll offset), `laneH` (vertical zoom), and a `playTime`.
- **Mapping:** a clip at `start` seconds, `len` long, renders at `x = (start - t0)*zoom`, width `len*zoom`. The whole content width is `duration*zoom`; horizontal scrolling moves `t0`.
- **Zoom around the cursor** (the correct UX): keep the time under the pointer fixed — on wheel, `t_new = (t0 + pointerPx)/zoom`, set `zoom'`, then `t0' = t_new - pointerPx/zoom'`. Likewise fit-to-view sets `zoom = viewportWidth/duration`, `t0 = 0`. **Zoom-out floors at fit** — you cannot zoom out past 100% view width, so the timeline always reaches the container edges; wider windows never show dead space past the last clip.
- **Snap grid** derives from `zoom`: pick the smallest "nice" tick step (`0.5/1/2/5/10/15/30/60/120/300` s) where `step*zoom ≥ ~64px`, and draw a hairline grid at that spacing. Clips snap to these ticks. Ruler labels use the same step so they stay aligned.
- **Vertical zoom** (`laneH`) scales lane row heights; the hero `fill` view grows naturally. Vertical scrolling if lanes exceed viewport.
- **Cheap on WebKit:** every frame, only the canvas content transform + a small readout change; never relayout the bar/ruler. The ruler is a separate fixed bar whose labels are translated by `-t0*zoom` (synced on scroll), so it never repaints the whole timeline.

### Interaction decisions (timeline)

- **Vertical tracks zoom with the timeline** — at the base (fit) zoom the tracks **fill the container height** (so they reach the end), and they grow taller as you zoom in; clips fill their own lane (never span multiple tracks). No separate row control — one unified zoom gesture. A vertical scrollbar appears only once the tracks outgrow the window.
- **Snap = grid + clip edges, both on.** Snap targets are the nearest grid tick *and* the left/right edges of other clips (same lane, plus cross-lane alignment). A thin **snap guide** line renders at the snapped position; the threshold is in pixels (~8px) converted to seconds by `zoom`.
- **Empty surface:** *click* places the playhead; *drag* is a **marquee** selection (a time × track rectangle that selects the clips it overlaps). Clip *drag* moves the clip with snap. This mirrors the actual canvas pointer model that the production implementation will use.
- **Clip edge drag → `length` (resize).** Grabbing a clip's right edge changes how long it runs; the clip's **fill rule** decides what fills the new span — `loop` repeats the loop span, `trim` shows more/less of the source. Snap-aware. (The `stretch`/time-warp fill rule is a deferred, locked feature request; v1 ships `loop` + `trim`.)
- **Loop-span drag → `loop_in`/`loop_out`.** A separate handle (the loop bracket) edits which part of the source loops, independent of the edge. Snap-aware.
- **DOM in the prototype, canvas in production.** The mockup uses DOM clips purely so the interactions are cheap to prototype and read; the shipped timeline, snaps, peaks, and meters are Canvas 2D per the hybrid strategy.

This is demonstrated live in [`mocks/arranger-profile.html`](mocks/arranger-profile.html) (data-driven clips in seconds, wheel/`+`/`−`/fit zoom around cursor, snap guide, marquee, click-to-place playhead, auto-fill + row-height, grid snap).

---

## 4. Responsiveness matrix

Desktop, keyboard+mouse only. No phone/tablet tier. Model = **density-aware scale**, not mobile breakpoints.

- **Resolution band:** 1080p (1920×1080) → 4K (3840×2160) → 8K (7680×4320). One `UI scale` factor (user-percent × device scale/DPR), applied to the root font/spacing via a CSS custom property. No per-breakpoint layouts.
- **Aspect bands:** standard (~16:9), wide (~21:9), superwide (~32:9). Wide/superwide reward horizontal room: `fill` grows, side panels hold their width → more timeline, not more empty panels.
- **Guarantee:** correct from 1080p at 100% up to 8K at 300%; nothing clips below the minimum usable size; top bar + timeline ruler pixel-stable across all bands.

---

## 5. UX / workflow principles (the purpose: work effectively with different audio tools)

1. **The surface is the work, not the decoration.** Default = focused, negative space, chrome on demand. High contrast between active/selected and idle.
2. **The timeline is the hero.** The creative model is generative — *the edit is the composition*, and the "performance" is a running modular/system output (evolving patterns over drones or stretched audio), not a human jam — so cut/paste/rearrange are primary and everything else recedes.
3. **Direct manipulation, no modal for the main gesture.** Drag/split/nudge clips with the primary tool. Dialogs are for secondary ops (import, plugin config). Keyboard first, mouse for precision.
4. **Progressive disclosure.** A profile surfaces only the commands it needs. No global "everything" menu. The profile is the rig.
5. **Undo/redo atomic and always visible.** Composition happens in the edit; undo is a first-class, always-on control. Not an afterthought.
6. **Thin, stable chrome.** Top bar fixed, ~44px, never scrolls/blurs. It should read as the "instrument," not furniture.
7. **Local, immediate feedback.** Transport clock on canvas; meters on canvas; nothing animates CSS that can fall behind the audio thread.

---

## 6. Default profile — `sound-arranger`

```
┌──────────┬──────────────────────────────────────────────┬──────────────┐
│ SOURCE   │ TRACK · RULER ·  TIMELINE (canvas, fill)     │ MIXER        │
│ POOL     │ lane  ▸▸▸▸▸  loop clips (edge + span handles)│  (toggle;    │
│ Project  │ T1 ▸▸▸▸▸    loop/once, fill rule             │   resizable) │
│ │        │ T2 ▸ ▸▸      snap·marquee·razor              │  ch1..chN    │
│ Library  │                                             │  per-track   │
│          │                                             │   color      │
├──────────┴──────────────────────────────────────────────┴──────────────┤
│ DETAIL VIEW  (contextual on focus: region · track effect-chain ·      │
│               source editor) — collapses when nothing is focused       │
└────────────────────────────────────────────────────────────────────────┘
```

**Top bar** (`~44px`, flat): brand · transport (play/record/stop) · tempo · undo/redo · snap/quantize · zoom-fit · profile switcher. Transport and undo/redo live **here**, not in the bottom slot.

**Views in this profile:**

- **Source Pool** (`left`) — two contexts: **Project** (this session's recorded/copied material) and **Library** (a referenced folder, default `~/Music/Samples`, overridable). Search, tag filters, in-place audition; drag items into the timeline, or *promote* a usable clip into the library. *(Project is self-contained (owns copies); Library is referenced/indexed.)*
- **Timeline** (`fill`) — the hero. Lanes are tracks; clips are loops (see the clip model below).
- **Mixer** (`right`) — toggleable + resizable. Track colors match the timeline lanes.
- **Detail View** (`bottom`) — contextual on focus (region | track effect-chain | source editor); collapses when nothing is focused.

### Clip model — everything is a loop

A clip is one type: `source · loop_in · loop_out · fill_rule (loop|trim) · mode (loop|once) · length · gain · fades`. There is no "region" type — a region is a clip with `mode = once`.

- **Edge drag → `length`.** What fills the span is the **fill rule**. (Time-warp/`stretch` is a deferred, locked feature request; v1 ships `loop` + `trim`.)
- **Loop-span handles → `loop_in`/`loop_out`**, independent of the edge.
- Sources carry **loop-point metadata** (`loop_in`/`loop_out`, `mode`, and later tempo/pitch) — a source-schema addition to schedule.

### Source model — Project vs Library

- **Library** — a referenced folder (default `$XDG_MUSIC_DIR/Samples`, user-overridable). Indexed, not copied.
- **Project** — self-contained. Using a library item **copies** it into the project; promoting a clip **copies** it into the library. Each copy carries a **provenance association** (metadata, not a live reference), so a project never breaks.
- Content-hash **dedup** is deferred (a locked lever); v1 uses plain copies.

---

## 7. Open questions

- **Comping / takes.** Ableton's Arrangement view shows multi-pass takes per track ("Take 4 / Take 2"), picked per section. Whether that fits our model better than cutting one long *run* is **deferred until we're producing music/sounds**; it's also in the frame for future compositional tools/algorithms. Not a v1 decision — recorded so it isn't lost.
- **Warp/stretch timing.** `stretch` is locked as a feature request but unscheduled; the source tempo/pitch metadata and the stretch DSP are a real step. Decide when it lands.
- **Split the Surface into a reusable primitive now, or keep it profile-defined until the second profile lands?**
- **Skins/theme switching via user token override, or a single fixed theme for v1?** (Proposing single theme, token-driven, to keep scope tight.)
- **Library indexing — resolved:** the browser **watches the library folder** for changes and re-indexes on change, so new/renamed/deleted files show up without a manual refresh. (This needs a filesystem watcher in the library browser, not index-on-open.)

## 8. Next steps

1. Build the two `mocks/*.html` as living references and lock the palette (done in this revision to `design-system.md`).
2. Scaffold the Vue shell: Top Bar (transport + undo/redo) + Surface slots + loader + registry + command/undo.
3. Add the first `ui-plugin`s (Source Pool [Project | Library], Timeline, Mixer, Detail View) as canvas + reka-ui hybrids.
4. Wire the `Bridge` to the Host API (`pool`/`peaks`/`meters()`/`providers_of`).
5. Schedule the source-schema addition for **loop-point metadata** (and later tempo/pitch), and the timeline **edge-drag resize + loop-span handles**.
