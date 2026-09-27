# Agent Note: the session rate, and where a source's rate is converted

Status: implemented

## Problem

Playing a 44.1 kHz WAV stopped the transport:

```
probe: pump error: clip 'c0' source is 44100 Hz but the session is 48000 Hz (rate-mismatched)
```

The refusal was deliberate — a rate-mismatched source would play ~8.8% flat with no counter, the
"clock dishonesty" the [output-negotiation
note](../feature/2026-09-10-audio-output-negotiation.md) rejects — but for a *clip arranger* the outcome was
wrong: the most common music rate (44.1 kHz, and equally 88.2/96 kHz) was unusable, and there was no
import door at all. The only path that put material in the pool, capture, resamples in real time (see
[wire-driftcompensator-into-capture](../feature/2026-08-24-wire-driftcompensator-into-capture.md)),
so a real resampler had been assumed and never written — the arranger's own message said so
("wire DriftCompensator or resample").

The report also asks the prior question: **what is 48 kHz for?**

## Why 48 kHz (the default)

- **The device is the clock.** 48 kHz is the default of modern interfaces, PipeWire/JACK, and the
  USB Audio class; a 48 kHz session is normally a straight passthrough. A 44.1 kHz session would
  renegotiate (or resample) on most hardware instead of removing a conversion — it moves the same
  problem to the mirror case.
- **Interchange.** 48 kHz is the video/broadcast/streaming rate (AES, EBU R128, ATSC, HDMI, Blu-ray,
  OBS/YouTube); 44.1 kHz is the CD/music-delivery rate. This tool's material arrives from hardware
  jams, phones, cameras, screen capture and other programs — 48 kHz is the meeting point, and it
  divides into the 96/192 kHz family most interfaces prefer for high-rate work.
- **Headroom.** 24 kHz of Nyquist leaves room for nonlinear processing (saturation, clipping) whose
  harmonics would fold back at 44.1 kHz, and it buys a cheaper reconstruction filter.
- **Arithmetic.** 48 000 = 2⁷·3·5³, so bars, beats, triplets and dotted values land on whole frames at
  ordinary tempos.

The cost is 8.8% more samples than 44.1 kHz — not a consideration. The rate is a **session**
property (one declared rate, `Engine::new(48_000, …)`); it is not a place to compromise, because the
pool now converts at the door.

## Decision

**The pool is session-owned material at the session rate, and a foreign rate is converted once, at
the pool boundary.**

- **`media::resample::Resampler`** — a band-limited polyphase resampler: a 128-tap Kaiser-windowed
  sinc (β = 8.6, ≈ -80 dB stopband) sampled at 512 fractional phases, each phase normalised to unity
  DC gain, cutoff at `min(in, out)/2` so the same kernel is an anti-imaging filter when upsampling and
  an anti-aliasing filter when downsampling. `process` emits only frames whose kernel window is fully
  fed; `flush` emits the tail against zero padding, so **the output does not depend on the block
  size** — the property that makes a converted take byte-reproducible. Equal rates are a bit-exact
  passthrough.
- **`Pool::import(src, session_rate) -> Import`** — bring a WAV of any rate in under its file stem: a
  byte copy when it already fits (a 16-bit source is not re-quantized), a single resample otherwise, and
  always a derived `.peaks` sidecar. A **multi-channel** file is split at this boundary into one mono
  source per channel (`{id}.ch0`, `{id}.ch1`, …), so the rate rule and the channel rule are both
  enforced once, where material enters ([stereo-material
  note](../../../../.agents/notes/implemented/feature/2026-09-24-stereo-material-per-channel-pool.md)). This is the
  path `--wave` now takes: the spike imports into its own session pool
  (`$TMPDIR/tui-shell-pool-<pid>`) instead of pointing `pool` at the user's directory.
- **`Pool::conform(session_rate)`** — the maintenance pass (sibling of `Pool::recover`) that makes a
  pool which predates the rules, or was filled by hand, playable. Each mismatched source is converted
  in place: the original is preserved as `{id}.wav.pre{rate}` (not a `.wav`, so the pool never indexes
  it as a source), the converted audio is written to a side file and **renamed over** the source, so a
  crash never leaves the pool without it. Per-source failures are reported, not fatal. A
  **multi-channel** source is expanded the same way — extra channels written beside it as `{id}.ch{k}`,
  `{id}` rewritten as channel 0 (backup `.pre{channels}ch` when only the split rewrote it), idempotent
  because the split leaves a mono source that no longer matches the filter.
- **`HostSession::set_pool` conforms as it adopts**, so every shell gets this without new vocabulary:
  the Tauri bridge, the CLI, and the spikes. The report is kept on the session
  (`HostSession::pool_conformed`) and rides `HostOutcome::pool_conformed`, so a shell can say what
  moved.
- **The arranger's refusal stays.** It is the invariant guard that keeps a clip a straight read in one
  frame domain; it is now unreachable through the pool, and its message names the remedy
  (`Pool::import`/`Pool::conform`) instead of describing machinery.

## Evidence

- `media::resample` (11 tests): exact length (44 100 frames in → 48 000 out), pitch (440 Hz
  zero-crossing count), unity DC, passband transparency (1 kHz residual < -60 dB; 20 kHz < -40 dB with
  level held), alias rejection (30 kHz at 96 kHz → 48 kHz attenuated below 5%), block-size invariance,
  bit-exact passthrough at equal rates, non-44100 pairs (48→96, 22.05→48).
- Throughput (`cargo test -p media --release -- --ignored --nocapture throughput`): **182× realtime
  release, 15× debug** for 44.1→48 kHz mono — a 30-minute take converts in ~10 s (release).
- `media` pool tests: import converts once and the audio is the same tone at full level; a matching
  rate is byte-identical; conform reports, preserves the original, leaves the index clean, and is
  idempotent; a broken file is reported without stopping the rest.
- `host` end-to-end (`adopting_a_pool_converts_a_foreign_rate`): a 44.1 kHz pool source is conformed
  on `Pool`, a second adoption is a no-op, and a clip spanning the converted source **bounces audible
  audio** — the reported bug, at the seam where it was hit.
- The spike (`--probe --wave /tmp/rate44.wav`): `imported rate44 — resampled 44100 → 48000 Hz, 48000
  frames`, then `ch0 = 0.5000`, `master = 0.3535` (0.5 × 1/√2, the centre-pan law) — audible, at the
  right pitch, through the mixer. Before: `pump error: … rate-mismatched`.

## Alternatives considered

- **Keep refusing and send the user to ffmpeg/sox.** Rejected: the most common music rate becomes
  unusable in a clip arranger, and the product still has no import door.
- **Resample on the render path** (a converter inside each `ArrangerNode` reader). Rejected:
  `Clip.src_len` is *both* the source window and the timeline span — one frame domain. On-read
  conversion splits them, so `Clip::end`, spans, fades, trim/split, `validate_clip`, lane drawing and
  the peak pyramid all grow a second domain (peaks are derived per source at the source rate and would
  have to be drawn through a ratio). Import-time conversion keeps one domain and the straight-read
  invariant, and keeps conversion off the audio path.
- **Resample in the device path** when the device cannot run at the session rate. Rejected: the same
  objection at a different seam, and the output-negotiation note already refuses to hide a mismatch
  there.
- **Reuse `DriftCompensator`'s linear interpolation.** Rejected: 44.1→48 kHz is upsampling, so
  nothing aliases, but linear interpolation's sinc² droop is ≈ -6 dB at 20 kHz and ≈ -1.5 dB at
  10 kHz — an audible dulling of cymbals and air in a music tool. `DriftCompensator` stays the
  *device-clock* mechanism; import needs a real resampler (as the capture note predicted).
- **Add a DSP dependency (`rubato`, `samplerate`).** The output-negotiation note assumed `rubato` was
  "already a dependency for stretch"; it is not, and no DSP crate is. Rejected for now: `media`'s habit
  is its own small, tested DSP over a C binding, and this is ~200 dependency-free lines. Revisit when a
  real *time-stretch* (a phase vocoder, not a resampler) lands.
- **Drop the session rate to 44.1 kHz** (the other reading of "what is the benefit of 48 kHz?").
  Rejected: it recreates the problem for hardware jams, cameras and video tools, and gives up the
  device default, the video interchange and the 96/192 family.
- **Let `pool` point at a music library and convert there.** Rejected: conform rewrites the source in
  place. The pool is a working directory; `--wave` imports into a session-owned one, and the user's
  original is never the file that gets rewritten.
- **A new `host v1` `import` command now.** Deferred: `set_pool`'s conform pass is what makes an
  existing pool playable and adds no log vocabulary. A user-facing import command (destination id,
  progress on a long take) is the next step.

## Consequences

- 44.1/88.2/96 kHz and mixed-rate pools play at the correct pitch; the arrangement keeps one frame
  domain (a clip is a straight read) and the render path still never allocates or converts.
- A pool is now declaratively **session-owned working material**: adopting it may convert files in
  place. The `pool` help text says so, and `--wave` imports a copy rather than using the user's
  directory.
- Conform is reversible by hand: the pre-conversion audio stays beside the source as
  `{id}.wav.pre{rate}`.
- A long import is a **blocking** pause inside `set_pool` (release ≈ 10 s for 30 minutes of mono,
  debug ≈ 2 minutes). Acceptable at load; a progress-reporting import command is the follow-up.
- `Pool::recover` (the crash pass) still has no production caller while `conform` now has one;
  folding both into a single "pool maintenance on adoption" call is the next cleanup.
- Deferred: an explicit import command; conform progress/cancellation; time-stretch/pitch (a different
  algorithm); a stereo *clip* (one source, two channels — material is per-channel now, the signal path
  is still mono per clip).

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-22.
