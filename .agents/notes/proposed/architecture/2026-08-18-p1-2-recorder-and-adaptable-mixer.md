# Agent Note: P1.2 — the recorder plugin, the adaptable mixer, and the USB-hardware-mixer target

Status: proposed

## Problem

P1.1 shipped the foundation and a **fixed-4-channel** soft mixer. The user requirement (2026-08-18): the mixer must be **adaptable to the inputs**, and the recording target is **USB audio via the hardware mixer** — the Soundcraft Notepad-12FX (gear note): class-compliant USB with **4 capture channels** (inputs 1 & 2 fixed, 3–4 a routable stereo pair), each USB channel one non-destructive source. Phase 1's recorder plugin (`cpal` → media pool, live peaks) must therefore discover the device's input layout, the mixer must mount with that channel count, and the capture must land as **per-channel float-WAV pool sources with a live peak pyramid** (the prior-art dispositions: 32-bit float on disk; 256-sample min/max reductions).

## Proposal

1. **The adaptable mixer (engine, `plugins::mixer`)** — channel count becomes a **mount parameter** (`channels`, 1..=8): the factory reads it, the node processes only that many inputs, and the profile sets it from the device's input config. The declared port surface covers the maximum (`ch0..ch7`, static — the graph's `MAX_AUDIO_INS`), so patches to channels beyond the mounted count are accepted but ignored (documented); the parameter surface declares all 8 channels. `MIXER_CHANNELS` (the default) stays 4; `MIXER_CHANNELS_MAX` = 8. The logged `Mount` params make the channel count replayable by construction.
2. **Float-WAV pool writer (media, `wav`)** — `WavWriter::create_float` (format 3, 32-bit float LE): the pool stores float sources per the research; 16-bit stays for bounce/export.
3. **Live peak pyramid (media, `peaks`)** — the Audacity structure: min/max per **256-sample base bin**, built **incrementally as samples arrive** (the "live" part), upper levels (×2 per level) computed at finalize; persisted per-source sidecar (`.peaks`: header + per-level min/max f32 pairs).
4. **The multi-channel capture path (media, `capture`)** — a `Capture` owns the interleaved device ring (fed by the cpal callback, or by a test's virtual device), demuxes to per-channel SPSC rings, and writes each channel to its own **float WAV + peaks sidecar in the pool**. `CaptureNode` (a graph `AudioNode`, `out("audio")` per channel) exposes a channel for monitoring: the profile patches `capture.ch{k} → mixer.ch{k}` — the adaptable mixer end-to-end.
5. **The recorder plugin (media, registered into the engine)** — `RecorderPlugin { channels }`: mounts one `CaptureNode` per channel; the profile routes them into the mixer. Capture start/stop stays a direct control-side `Recorder` action in P1.2 (the *composition* — which clips exist — is logged in P1.3; the pool capture is I/O, not a log-replay concern).
6. **Device discovery (media, `devices`)** — `default_input_config()` reports the input device's channel count + sample rate; the real-hardware test opens the default input, captures a short multi-channel pass, and asserts the per-channel float WAVs + peaks land in the pool (this machine's Scarlett 2i2 is 2-channel; the Notepad-12FX's 4-channel layout is verified when the box arrives — gear note checklist).

## Alternatives considered

- **Keep the mixer at 4 and hardwire the Notepad's layout** — fails "adaptable to the inputs": the Scarlett (2ch), the Notepad (4ch), and later interfaces all differ. Rejected.
- **Dynamic ports (non-static `ports()`) at runtime** — a real trait change for no current need; the mount-time `channels` param over a static max surface is the honest Phase-1 shape (runtime reconfig is a later refinement).
- **One input node with many audio Out ports** — the graph enforces one audio Out; N per-channel `CaptureNode`s fit the existing plugin/graph model. Chosen.
- **Record the mono master only** — loses the Notepad's whole point (per-channel multitrack). The capture is per-channel; the master is the monitoring/export path.
- **Stems/MIDI instead of pool WAVs** — the pool is the platform's media substrate (minimal-core note: sources referenced by content hash); per-channel float WAVs ARE the pool. Chosen.

## Acceptance criteria

- The mixer mounts with `channels = N` (1..=8), processes exactly N inputs, and the mount is replayable (byte-identical render with a differing `channels` param).
- A virtual 4-channel device captured for 2 s lands as **4 float-WAV pool sources + 4 peaks sidecars**; reading the WAVs back matches the source within float-roundtrip tolerance; the base-level peaks match an independent min/max computation (256-sample bins).
- The 4 `CaptureNode`s patched into `mixer(channels=4)` produce the summed master; a 2-channel variant (Scarlett) mounts `channels=2`.
- The real-hardware device test captures the default input for a moment and writes its channel-count WAVs + peaks (skips politely without a device).
- Counting-allocator test covers a capture→mixer graph; clippy clean; workspace tests green.

## Risks

- **cpal multichannel capture variance** (devices deliver 2, 4, or more channels, formats vary) — the capture path is format-generic (interleaved f32), device-specific quirks stay in `devices.rs` (Spike-B discipline).
- **The pool format decision hardens here** — float WAV + `.peaks` sidecar is the media pool's format; content-hash naming and the pool directory layout are finalized in P1.3 with the clip editor (recorded).
- **Peak sidecar size** — ~4.5 MB for a 25-min mono take; fine, and the base level alone (live) is ~2.25 MB.
