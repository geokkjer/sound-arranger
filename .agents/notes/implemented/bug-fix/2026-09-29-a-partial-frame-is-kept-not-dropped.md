# Agent Note: A partial frame at stop is kept, not dropped

Status: implemented

## Problem

The capture demuxes an interleaved ring into per-channel pool sources, and it processes **whole
frames only**. Mid-batch that is deliberate and correct — a partial frame is carried to the next
batch, because discarding it would shift every channel by a sample (a bug the e2e test caught once
already).

At **stop** there is no next batch. The old code turned a leftover partial frame into an error
(`capture stopped with a partial frame (1/2 samples) dropped`), which surfaced through
`Capture::stop`, failed `record stop`, and **discarded the tail**. Two costs, and the second is why
this is a bug rather than a nicety:

- the samples the other channels *did* deliver were thrown away — for a recorder, a sample of silence
  is better than material lost;
- **whether a stray sample happened to be in the buffer at stop was pure timing**, so the condition
  was a flake. It is the flake in the capabilities list: `a_take_records_into_the_pool_and_plays`
  failed intermittently in full-suite runs, passed in isolation, and failed **2 of 2** CI attempts on
  one commit once required checks made it visible and blocking.

## Decision

**Keep the tail: pad the channels that never delivered with silence, and process it as one frame.**

- `n_frames == 0 && stopping && !buf.is_empty()` pads the buffer to a whole frame
  (`buf.resize(channels, 0.0)`) and processes it, instead of erroring and breaking. The buffer is
  drained by `n_frames * channels`, the same count in both paths.
- **Silence is the honest value** for a channel whose sample never arrived, and the samples that did
  arrive are real material — which is what a recorder owes its input.
- **The per-channel stems stay the same length**, which the
  [capture-topology note](../../proposed/architecture/2026-09-25-capture-topology-aligned-stems.md)
  requires: writing only the channel that delivered would leave the stems mismatched, and alignment
  is measured on material assumed to be aligned.
- **The `take` declaration does not change.** `dropped` means "source frames the ring could not
  hold" — a different fact. A padded sub-frame tail lives in the *material* (one frame of silence in
  the missing channel), not in the declaration.
- Regression test, deterministic and race-free:
  `a_tail_that_never_completed_a_frame_is_padded_not_dropped` feeds five whole stereo frames plus one
  stray left sample, stops, and asserts that `stop()` succeeds, the take is six frames, the stems are
  equal length, the delivered sample survives, and the channel that never delivered reads `0.0`.

## Alternatives considered

- **Count it as dropped and keep erroring** (my first suggestion, and wrong): it still throws
  material away, and it keeps a benign timing condition fatal. The user's framing was better —
  prefer a sample of silence to losing a sample.
- **Write the partial sample only to the channel that has it.** Rejected: unequal per-channel stems
  are exactly the misalignment the stems model exists to prevent.
- **Add a `padded` count to the `take` declaration.** Deferred: a `host v1` format change for a
  sub-frame fact whose result is already visible in the material. If a device ever drops whole
  *frames*, that already belongs in the declaration as `dropped`.
- **Ignore the tail silently (break with no error).** Rejected: that is the old behaviour minus the
  honesty — material thrown away, nothing said.
- **Pad at the device/ring boundary instead of in the demux.** Rejected: that ring also feeds
  monitoring and is drained by the device callback, so padding there would change what a monitor
  hears for a condition that only matters while a take is being finalized.

## Consequences

- **The flake is gone, not papered over** — the timing dependence *was* the error path. Fifteen
  consecutive local runs of the host test: 0 failures. The deterministic unit test covers the
  condition directly, so a reintroduction cannot hide behind timing again.
- Mid-stream behaviour is unchanged: the tail is still carried to the next batch, and the drain count
  is equivalent (`n_frames * channels` in both paths).
- A device that stops mid-frame now yields one extra frame whose missing channels are silence. That
  is not recorded anywhere: the amount is sub-frame by construction, and the material shows it.
- The comments at the site state *why* the tail is kept — both at the batch boundary and at stop — so
  a future reader does not "simplify" it back into an error.

*Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-29.*
