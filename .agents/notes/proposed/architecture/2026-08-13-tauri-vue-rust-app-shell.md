# Agent Note: App shell — Tauri v2, Vue 3, TypeScript, and the cdp-front UI stack

Status: proposed

## Problem

sound-arranger pairs a low-latency Rust audio engine with a bespoke DAW-style UI, so the shell choice has to serve both. The project already owns a proven frontend stack in `cdp-front` (reka-ui + shadcn-vue + Tailwind v4 + rolldown-vite), and RESEARCH.md §4 notes the decisive fact: the UI library only styles panels, toolbars, and dialogs, because the timeline is canvas — so the library choice has almost no bearing on timeline performance. RESEARCH.md §3/§4 locked the shell; this note owns the decision and its acceptance criteria.

## Proposal

- **Tauri v2** (Linux WebKitGTK, macOS WKWebView, Windows WebView2) with **Vue 3.5 + TypeScript** (`<script setup>` SFCs).
- **Reuse the cdp-front stack**: reka-ui + shadcn-vue + Tailwind v4; rolldown-vite is optional (Tauri only calls the `dev`/`build` scripts).
- Frontend talks to Rust only through `@tauri-apps/api/core` `invoke()` / `listen()` / channels; permissions are deny-by-default via `src-tauri/capabilities/*.json`. No `tauri` v1 module.
- **Timeline renders on canvas** (Rule 0: never clips as DOM nodes): layered static/clip/overlay passes, per-clip offscreen caching and blitting, peak pyramids (LOD buckets in Rust), viewport culling, dirty-flag `requestAnimationFrame`. WebGL/PixiJS only later, for per-clip spectrogram overlays.
- **IPC is control-only JSON**: `invoke()` commands carry control payloads; audio samples never cross IPC (peaks arrive downsampled); transport position is a local frontend clock with sparse (~10 Hz) resync; meters/progress via throttled `emit` (~30 Hz).

## Alternatives considered

- **Naive UI** — a fine batteries-included fallback but redundant: the headless primitives already cover what a bespoke DAW needs.
- **PrimeVue** — added a license-manager/premium tier and is more opinionated than a DAW wants.
- **Vuetify** — Material-locked; the wrong visual language for a tape editor.
- **Element Plus** — dated and heavy.
- **DOM-per-clip timeline** — janks at a few thousand clips; canvas with viewport culling is the safe default.

## Acceptance criteria

- The phase-0 spike (x86 Linux) runs: Tauri v2 + Vue from the cdp-front stack, a canvas timeline drawing a few hundred clips, and cpal playing a file — validating rendering, IPC, and the audio skeleton end-to-end.
- Frontend uses only `invoke()` / `listen()` / channels from `@tauri-apps/api/core`; capabilities are deny-by-default; no audio samples cross IPC.

## Risks

- rolldown-vite + Tauri dev-server port wiring needs confirmation (Tauri just calls the scripts).
- Tauri + Nix friction exists but is minor on x86 — plain system deps work and Nix can stay optional.
