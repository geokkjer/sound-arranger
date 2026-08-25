# Agent Note: Vue host-side plugin runtime — the core shell, profiles, and ui-plugins (the Tauri v2 UI layer)

Status: proposed

## Problem

The Host API note fixed the core↔host seam: engine plugins are in-process (`Plugin` trait), host/UI plugins go through the Host API (`crates/host`), and the headless reference host proves the contract. Its "Next" line is the Tauri v2 shell. But *how* the Vue frontend should be structured has never been decided — and it's easy to get wrong in a way that repeats the Ardour anti-pattern the Host API note exists to avoid: letting the session model and a specific GUI grow together. We need a deliberate answer for what the UI *is* before writing it.

The user's framing is "everything is a plugin in the UI too": a core shell (mostly empty, but a top bar is needed), profiles as collections of plugins, and plugins as building blocks. The constraint that must govern the whole design is the renderer.

## Proposal

**The UI is the host side of the Host API, structured as a composable shell + plugin runtime.** Two channels, never one — this confirms (not contradicts) the Host API note.

1. **Core UI (always present).** A top bar (~44px, flat, solid — no blur) + a **slotted Surface** (fill/left/right/bottom) + the runtime (declarative loader, registry, command dispatch, atomic undo/redo, theming, clock/transport scaffold). Chrome primitives are components, not plugins.
2. **`ui-plugin`** (Host API, control-rate, cross-boundary). Contributes any of: `View` (occupies a Surface slot), `Command` (top-bar action + binding), `Overlay` (transient: marquee, inspector, splitter), `State`, `Bridge` (a declared mapping to engine plugins/params — not one shared interface), `Preference`. **`engine-plugin`** stays in-process (`Plugin` trait) and unchanged.
3. **`profile`** = an ordered collection of plugins + `Layout` (views → slots) + `Keymap` + `DefaultState` + `Wiring`. **A profile is itself a plugin**, so profiles compose. Switching profiles = switching instruments.
4. **Rendering/component strategy — hybrid.** Headless base primitives (`reka-ui`: menus, popovers, dialogs, tooltips, combobox, scroll-area) for behavior/a11y only; **custom Canvas 2D** for anything that repaints at audio/drag rate (timeline + clips, waveform peaks, meters, knob curves, transport clock, playhead); thin hand-rolled CSS for the perf-critical chrome; Tailwind v4 for layout utility only, never the hot path. The timeline is a **fixed time-space through a scrolling window** — zoom is a pixels-per-second mapping change (round-cursor + fit), not a per-clip relayout; the ruler is a fixed bar whose labels translate by `-t0*zoom` (viewport/zoom spec in [ui-plan.md](../../docs/design/ui-plan.md)). Timeline interactions: **snap = grid + clip edges (with guide), empty-click places playhead, empty-drag is marquee selection, clip-drag moves with snap; track height scales with the horizontal zoom (no separate row control), clips fill the lane**. DOM is prototype-only; production is canvas. **Pushback** (recorded so it's not re-litigated): the "mostly empty square" is the *shell before a profile loads*; the loaded profile is full. A single plugin interface for both engine and UI would be a leaky abstraction — headless batch renders (no UI) and viewless DSP nodes (no audio) would be forced to satisfy dead contracts.
5. **The renderer is the binding constraint.** WebKitGTK (`webkit2gtk-4.1`, 2.52.6 pinned per CachyOS/Arch): no `backdrop-filter`/`blur`, no large `box-shadow`, no `will-change` on many nodes, no DOM-per-clip or DOM-per-waveform column, and never relayout the bar on a transport tick. Modern CSS (`oklch`, `color-mix()`, `@property`) is safe at this version — use it in the theme layer, keep the render loop plain. The engineer/minimalist aesthetic is *also* the correct performance choice: fewer layers, flat surfaces, canvas for anything moving.
6. **Responsiveness = density-aware scale, not mobile breakpoints.** Desktop keyboard+mouse only. 1080p→4K→8K via a single CSS-variable UI scale (user percent × device scale/DPR). Aspect bands standard/wide/superwide: `fill` grows, side panels hold a set width → wide/superwide yield *more timeline*, not more empty panels.
7. **UX/workflow contract** (the whole point is working efficiently with different audio tools): surface-is-the-work; timeline is the hero (the edit is the composition); direct manipulation with no modal for the main gesture; progressive disclosure (a profile surfaces only its commands); atomic always-visible undo/redo; thin stable chrome; local immediate feedback (canvas clock/meters, never CSS animating behind the audio thread).

Working docs: [`docs/design/ui-plan.md`](../../docs/design/ui-plan.md) (shape + tradeoffs + responsiveness), [`docs/design/design-system.md`](../../docs/design/design-system.md) (token contract). Mockups: [`docs/design/mocks/core-shell.html`](../../docs/design/mocks/core-shell.html), [`docs/design/mocks/arranger-profile.html`](../../docs/design/mocks/arranger-profile.html).

## Alternatives considered

- **One plugin interface for engine + UI.** Rejected: a graph node and a DOM view are different contracts; a single interface forces dead methods on headless (batch) or viewless (DSP) plugins. The agent-note discipline says keep the boundary honest; two channels + a `Bridge` does that.
- **reka-ui + shadcn-vue + Tailwind v4 fully.** Rejected as the load-bearing stack: lots of utility DOM and CSS compositing cost on WebKitGTK. Kept as *headless primitives + layout utility only*.
- **Fully custom components, no library.** Rejected: re-implementing focus traps/ARIA for the base primitives is pure cost; the value is in the audio-native canvas widgets, which are custom anyway.
- **Electron** (Chromium, nicer CSS). Rejected by the Host API note already (heavyweight). The engineer/minimalist aesthetic is chosen *because* it makes WebKitGTK's constraints work.
- **Shadows/backdrop-blur for "premium" chrome.** Rejected: the known WebKitGTK compositing cost. Hierarchy via surface tone + hairlines instead.
- **Mobile/touch layout, breakpoint-based responsive design.** Rejected: desktop keyboard+mouse only; density-aware scaling, not breakpoints.

## Acceptance criteria

- Loading a profile produces exactly the views its `Layout` declares, in the Surface slots, reflowing on window resize (fill absorbs remainder; wide/superwide → more timeline).
- Adding/removing a ui-plugin reflows the Surface without a full reload or a broken layout.
- No mockup uses `backdrop-filter`, `blur`, or a large box-shadow; the top bar is flat and stays pixel-stable across a transport tick (no bar relayout; the clock paints a small region).
- Timeline/peaks/meters/knob-curves render on canvas; the render loop contains no utility-CSS re-layout; DOM node count does not grow with clip count.
- Layout is correct from 1080p @ 100% up to 8K @ 300% and across 16:9 / 21:9 / 32:9, with nothing clipping below the minimum usable size.

## Risks

- **WebKitGTK version creep.** Pinning 2.52.6 means an older distro WKGTK could regress; mitigations: the version is a pinned/devenv fact, and modern-CSS usage is confined to the theme layer.
- **Over-abstracting the plugin runtime.** Two channels + manifest is real work; scope creep toward "plugin-for-everything" (making every button a plugin) is explicitly out of scope — base primitives are components.
- **Canvas complexity.** The timeline canvas (peaks, selection, marquee, snapping) is the hardest piece; it's the reason the shell is structured as it is, and it's where most of the risk sits.
