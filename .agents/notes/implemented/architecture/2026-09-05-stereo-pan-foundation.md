# Agent Note: Stereo + pan — the mixer is a stereo master (step 1 of the acid-arranger resume)

Status: implemented

## Problem

The substrate was **mono end-to-end** — a graph audio port was one flat `[f32]`
stream, PDC was a mono `RingDelay`, the mixer mixed mono channel inputs into a
mono master with no pan, and `media`/bounce wrote mono WAV. This blocked the
aligned next step of the acid-arranger resume (review 2026-09-05): a credible
mixer, stereo sources/clips, and mastering all need stereo. Pan was previously
deferred ("pan arrives with stereo"); that deferral is now resolved.

## Decision

The audio path is **channel-aware**, mono by default, and the mixer is the
stereo master. Concretely, as shipped:

- **`Port.channels`** (`crates/engine/src/graph.rs`): audio ports carry a channel
  count (1 mono default, 2 stereo); `Port::audio`/`stereo_audio` constructors and
  a `Port::channels()` accessor. Control/trigger/note ports stay 1.
- **Graph is channel-aware** (`graph.rs`): per-node audio output buffers are sized
  `channels * BLOCK`; `Graph::out_channels()` reports the master width; the render
  loop computes the block frame count as `out.len() / channels`, so a stereo
  master does not halve the frame count; the master copy writes all channels.
  Fan-in stays mono for step 1 (every connection is mono→mono; a stereo
  connection is the next sub-step, with the per-port channel count as the seam).
  Mono nodes are unchanged — a mono node writes the `BLOCK` slice,
  byte-identical to before.
- **The mixer is the stereo master** (`crates/engine/src/plugins/mixer.rs`): the
  master audio out port is `channels: 2`; the `MixerNode` writes interleaved L/R
  and applies a per-channel **equal-power pan** (`ch{i}.pan` in `[-1, 1]`: `-1`
  hard left, `0` center = √2/2 each, `+1` hard right). Per-channel gain/mute/solo
  are unchanged; `master.gain` scales both channels; the master meter is
  `max(|L|, |R|)`. `MIXER_PARAMS` gained `ch{i}.pan`.
- **Engine render is channel-aware** (`render.rs`): `render`/`render_into` size
  the buffer by `out_channels` and chunk by `BLOCK * channels`; the clock advances
  by frames, not samples. `channels_for_render` flushes scheduling events due at
  the current frame first, so a scheduled mixer mount (frame 0) makes the master
  stereo for the whole render instead of silently halving the frame count.
- **Stereo bounce**: `media::bounce` reads the channel count *after* `render`
  flushes (a scheduled mount changes the width) and writes a 2-channel 16-bit WAV;
  the reference host's `Bounce` handler writes the master's channel count too. A
  mono source on a center-panned channel yields a stereo WAV whose channel 0
  carries `source * √2/2`.

A mid-render *master-width* change is handled by **parking** it at a render-call
boundary (`Engine::changes_master_width`; `render_block` parks a mixer
Mount/Unmount the way arrangement ops are parked): a fixed-size output buffer
cannot represent two channel widths in one call, so the width is constant per
call and the change applies at the next `flush_scheduled`. This keeps the frame
count exact — a replayed log rendered in one call across a mid-session mixer
mount advances the clock by exactly the requested frames (regression test
`replay_across_mixer_mount_in_one_call_advances_the_clock_exactly`). The width
change taking effect at the call boundary, not its exact frame, is the documented
trade-off.

## Consequences

- The mixer produces a genuine stereo master from mono via pan, and bounces are
  2-channel; the CLI smoke (`pool → arrange → bounce`) writes a 2-channel,
  48000-frame WAV.
- Mono graphs remain byte-identical: channel count 1 is the default, and the
  existing graph/mixer/arranger/bounce suites pass unchanged once the port field
  is set.
- Stereo *sources/clips* are still forthcoming — a genuine stereo take/clip is the
  next sub-step (stereo `ArrangerNode`/pool source read via the `channels` seam).
  Mono-to-stereo pan is the shipped first milestone.
- The pan law is fixed equal-power (a taste constant can be added later).
- `cargo test --workspace` is green; clippy is free of errors (the remaining
  style lints are `chunks_exact_to_as_chunks` suggestions in the test
  de-interleave helpers).

## Alternatives considered

- **Interleaved stereo throughout** (`[f32]` of `BLOCK*2`): matches WAV/DSP, but
  PDC and per-port fan-in would need interleaved-aware code and every mono node
  would stride. Rejected as higher-churn for this planar-by-design engine.
- **Stereo as two mono audio-out ports (per-node two outs):** violates the
  Phase-1 "at most one audio Out" rule; L/R would flow through two cords not one
  typed port. Rejected.
- **Keep the graph mono; make only the master/WAV stereo at the mixer:** smallest
  diff, but leaves `NodeIO`, PDC, `audio_out` mono, so stereo sources and future
  stereo effects would have no seam. Rejected as a dead-end.
- **Native stereo sources/clips now instead of mixer-first:** bigger; the mixer
  should be the foundation first. Rejected for step 1 — the `channels` field is
  the seam for native stereo clips next.

*Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-05.*
