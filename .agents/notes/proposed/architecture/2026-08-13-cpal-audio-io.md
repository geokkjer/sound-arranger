# Agent Note: Audio I/O via cpal — in and out, realtime-safe

Status: proposed

## Problem

Recording live jams and playing the arrangement back is the core of the tool, so latency and reliability of the audio path decide the product. The audio callback must never allocate or block. RESEARCH.md §5 locked cpal for both input and output with a realtime-safe handoff; this note owns the decision.

## Proposal

- **cpal (0.18.x) for input and output**: ALSA `hw:` or JACK on Linux (avoid PulseAudio on the critical path), CoreAudio on macOS, WASAPI on Windows.
- **Realtime-safe handoff**: `rtrb` SPSC ring buffer + `basedrop` for allocation-free memory between UI and audio threads; `audio_thread_priority` raises the callback thread on Linux.
- **Recording**: cpal input → `hound` WAV into the media pool, with live waveform peaks.
- **rodio** only as an optional quick playback bootstrap, dropped once cpal direct playback is in.

## Alternatives considered

- **rodio as the primary I/O** — playback-focused with no input path; rejected for the core, acceptable as a bootstrap.
- **PulseAudio on the critical path** — latency too high on Linux; use ALSA `hw:` or JACK.
- **JACK-only** — Linux-only, excludes macOS/Windows; cpal gives all three backends behind one API.

## Acceptance criteria

- Phase-0 spike: cpal plays a file.
- Phase-1 record path: cpal input → hound WAV with live peaks; the callback never allocates or blocks (rtrb/basedrop); RT priority applied on Linux.

## Risks

- ALSA `hw:` device selection varies by hardware — the device picker needs a sane fallback list.
- Backend differences (WASAPI vs CoreAudio vs ALSA) need per-OS testing for the record path.
