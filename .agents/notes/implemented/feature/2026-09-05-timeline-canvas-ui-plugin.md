# Agent Note: Timeline canvas — the first clip-arranger ui-plugin

Status: implemented

## Problem

The objective's step 2 is to wire the scaffolded Tauri/Vue shell to the Host API
so the profile is *usable*. The Rust bridge (run_host_script → ScriptOutcome with
the serialized arrangement) and the incremental session API already exist; the
Vue side was still the placeholder "Load a profile" surface. The timeline is the
hero view (the "edit is the composition" model), so the first ui-plugin is a
timeline canvas that draws the arrangement value.

## Decision

- **`crates/shell/src/TimelineCanvas.vue`** — a canvas ui-plugin in the hero slot:
  it draws the arrangement (tracks as lanes, clips as blocks positioned/scaled by
  `at_frame * pxPerFrame`, `src_len * pxPerFrame`), a ruler, and lane labels. The
  clip colour keys off the source id; fade/loop fields are carried in the value.
- It **wires the Host-API bridge**: a "Run in host" button invokes
  `run_host_script` (Tauri `invoke`, arg `scriptText` → Rust `script_text`) with a
  script box, and draws the returned `arrangement` when it has tracks. A demo
  arrangement is the fallback when the host produces none (headless/no-pool).
- **`crates/shell/src/App.vue`** — the surface now hosts `TimelineCanvas`.
- `@tauri-apps/api/core` (the `invoke` source) is already a dependency; no new
  deps. The bridge's `ScriptOutcome.arrangement` (a serialized `media::Timeline`)
  is the data contract — no second round-trip.

## Consequences

- The profile has a first real, visible view: `pnpm typecheck` and `pnpm build`
  both pass (17 modules, ~67 kB), so the canvas compiles and bundles.
- It is verifiable headless only at build level (a Tauri GUI needs WebKit/GTK and
  a window); the live clip data path is exercised by the existing host tests.
  The demo arrangement gives the canvas content without a media pool.
- Follow-ups (next ui-plugins): **source pool** and **mixer** panels, and
  **interactive editing** (drag/razor-split) through the incremental session API.

## Alternatives considered

- **Plain DOM per clip instead of a canvas:** the ui-plan locks canvas for
  something that moves at drag/audio rate; DOM-per-clip is a WebKit
  compositing/relayout cost. Chosen canvas.
- **Return only clip metadata, not the tracking positions, to the frontend:** the
  canvas needs `at_frame`/`src_len` to draw; the bridge already returns the whole
  value. Chosen whole-value.
- **Wait for a full profile/plugin system before any view:** the plugin/registry
  framework is a larger step; a single concrete ui-plugin proves the shell↔bridge
  data path and is incremental. Chosen.

*Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-05.*
