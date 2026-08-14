# Agent Note: Engine as a standalone Rust crate, not Tauri-coupled

Status: proposed

## Problem

The engine must stay usable if the product ever grows a headless/ARM "box mode" (Pi 5 + physical panel, same Vue UI served over LAN). RESEARCH.md §2 keeps that as a separate later phase and explicitly says *don't design for it now* — but coupling the engine to Tauri now would foreclose it later without a rewrite. The engine is also the part most worth testing without a webview.

## Proposal

- The engine lives in its own standalone Rust crate (not Tauri-coupled): it owns its own thread, the transport + real-time mix in the cpal callback, recording (cpal input → hound WAV into the media pool), and peak generation.
- The Tauri layer is a thin host: control-only IPC over `invoke()` / `listen()` / channels; audio never crosses IPC.
- No ARM/Pi-specific code in phase 1; no separate box-mode design now.

## Alternatives considered

- **Single Tauri-coupled binary** — simpler initially, but forecloses the headless box mode without a rewrite.
- **Designing the full box mode now** — pre-optimization; the phase-1 goal is record → cut → arrange → mix on x86.

## Acceptance criteria

- The engine crate compiles and its tests run with plain `cargo test` — no Tauri or webview involved.
- A headless smoke binary can record and bounce without the frontend.
- No Pi/ARM-specific dependencies in phase 1.

## Risks

- The engine/host seam needs defining early enough that the Tauri layer stays thin — but keep the seam minimal (YAGNI); the IPC command list is the contract.
