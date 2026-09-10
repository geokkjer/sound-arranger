# Agent Note: audio output — negotiated device stream and a device-paced pump

Status: implemented

## Problem

The shell was **silent**: the live pump rendered the engine master into the void. `media` already had
`devices::open_output`, but it could not be trusted with a real session:

- **Clock dishonesty (the Tauri review's F6).** It used `device.default_output_config()`, so a 48 kHz
  session on a 44.1 kHz-default device would play ~8% flat with **no counter** — the loudest form of
  the pitch/speed lie the arranger otherwise refuses carefully (it rejects a 44.1k source in a 48k
  session).
- **Mono-only ring.** `fill_output` popped *one* sample per device frame and duplicated it, so a
  stereo master (the mixer's L/R bus, now that pan has landed) would be mis-mapped.
- **Silent starvation.** An empty ring played silence with no visible count.

## Decision

- **`open_output(ring, source_channels, requested_rate)`** negotiates the config: it scans
  `supported_output_configs()`, prefers **F32** then the **most channels** among the ranges whose
  `[min, max]` contains `requested_rate`, and runs at the session rate. If no range supports it, it
  falls back to the device default and sets **`rate_mismatch`** — the shell surfaces that instead of
  playing at the wrong speed. `best_output_range` is pure, so the preference order is unit-tested
  without a device.
- **`fill_output` maps one source frame per device frame** (`source_channels` interleaved samples):
  passthrough when the counts match, duplicated from a mono source, averaged for a mono device
  (never silently dropping a channel). A **partial source frame is never consumed**, so L/R stay
  aligned across a starve.
- **Counters.** Every starved frame increments `OutputHandle::underruns`; the mirror of the input
  path's `overruns`. `OutputHandle` also carries the `sample_rate`, `channels`, and `requested_rate`,
  and exposes `play()`/`pause()` so callers never need cpal in scope.
- **The live pump feeds it (`host::live`).** `HostHandle::spawn_with_audio` opens the device inside the
  actor thread (the cpal `Stream` is `!Send`, so it stays there) and the pump fills a stereo ring to
  ~half full; the callback draining it is the timing signal, so the pump is paced by the **device**
  clock, not the wall clock. The stream is **paused while stopped** (so an idle host accrues no
  underruns) and its ring is drained on stop/load (no stale tail replays). `spawn()` stays silent for
  tests. `Snapshot::audio` publishes the negotiated rate, `rate_mismatch`, `underruns`, `drops`, and
  an `error` when the device cannot be opened at all — silent, never a hard failure.

Tests: mono duplication, stereo passthrough, stereo→mono downmix, empty-ring silence counting
underruns, partial-frame starvation keeping alignment, zero-channel degradation, and range selection.

## Alternatives considered

- **Resample** when the device cannot run at the session rate. A real DSP job (rubato is already a
  dependency for stretch), and not needed for the common 48 kHz case. Deferred; we surface the
  mismatch instead of hiding it.
- **Refuse to open** on a rate mismatch. Harsher than needed — a user may still want to hear it — so
  `rate_mismatch` is reported and the caller decides. Rejected for the media layer.
- **Keep the mono source ring.** Simpler, but throws away the stereo master and pan. Rejected.
- **Count underruns per missing sample.** Noisy and not the audible unit; a starved *frame* is.
  Rejected.

## Consequences

- The device path is stereo-capable and **clock-honest**: it either runs at the session rate or says
  it could not.
- The counters are what the live runtime and the shell will surface (next unit).
- The shell surfaces it: the bridge opens the device by default
  (`HostHandle::spawn_with_audio`), `TransportState` carries the negotiated rate / mismatch /
  counters, and the top bar shows a small readout (`♪ 48.0k`) — amber with a rate-mismatch warning,
  red with the reason when audio is unavailable, counters in the tooltip.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-10.
