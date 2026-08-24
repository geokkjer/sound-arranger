# Agent Note: wire the DriftCompensator into the capture path

Status: implemented

## Problem

`DriftCompensator` (media, Spike B) was **dead code** — only tested in isolation, never wired
into a real path. A recording input device's clock drifts from the session clock; over a
20-min jam the difference is thousands of samples, so the recorded take is not in session
frames. GLM-5.3's review called this a "major": no layer reconciled sample rates.

## Decision

The `DriftCompensator` is wired into the **capture demux** (its natural home, off the audio
path, where `Vec` buffering is acceptable):

- `Capture::start` gains an **`input_rate`** param (the device clock) alongside the existing
  `sample_rate` (the session clock). The demux thread creates **one `DriftCompensator` per
  channel** (`DriftCompensator::new(input_rate, sample_rate)` — mono, so one per channel),
  feeds each channel's extracted device frames via `push_input`, and pulls session frames via
  `pull_output` into a **sized** buffer. The session frames are written to the pool WAV (declared
  at the session rate) and to the monitoring rings. `input_rate == sample_rate` is a
  bit-exact passthrough.
- The session buffer is sized `ceil(n_frames / ratio) + 2` to cover **both** drift directions
  (a `n_frames`-sized buffer caps the `ratio < 1` case, drifting the take + an unbounded
  `pending` leak — found by kimi); `frames()` accumulates the session frames actually pulled.
- `pull_output` clamps the consumed count to `pending.len()` — a fractional `pos` carried into
  a short (take-tail) batch could `drain(..floor(pos))` past the buffer.

The pool WAV is at the session rate, so a captured take is **never** refused by
`ArrangerNode::new`'s playback rate-mismatch check (that refusal is for an *imported source
WAV* at a different rate, which needs a real resampler, not drift linear-interp).

## Alternatives considered

- **Leave `DriftCompensator` in the input-device callback** — rejected: it buffers in a
  `Vec`, which allocates (not no-alloc, and the audio callback is the RT path). The demux
  thread is off the audio path.
- **A `n_frames`-sized session buffer** — rejected (kimi): it caps the `ratio < 1` direction.
- **Refuse `input_rate != sample_rate`** — rejected: that would make the compensator dead
  again (every caller passes equal or is refused); drift compensation is the correct handling
  of a real device-clock drift.

## Consequences

- A drifting input records into **session frames** (both directions), and the take pitch is
  preserved. `drift.rs` is no longer dead code.
- 2 new capture tests (`faster`/`slower` drift, asserting the session-frame count + pitch).
  18 workspace suites green, clippy clean.
- **Deferred**: adaptive (buffer-level feedback) ratio; a true tail-drain on stop; the real
  audio-device caller passing the actual stream rate; a poll-with-deadline test instead of the
  fixed sleep; an explicit passthrough bypass (bit-exact already at ratio 1).
