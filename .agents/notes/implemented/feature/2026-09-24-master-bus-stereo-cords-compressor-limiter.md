# Agent Note: the master bus — stereo cords, a compressor + lookahead limiter, and latency-aligned renders

Status: implemented

## Problem

The owner's alpha ask ends with "**export the final mix (with simple mastering tools like
compressor on the mix out)**". The plan's item 12 fixes the shape: a `master` plugin mounted
*after* the mixer that claims the graph's output, so the live pump and the bounce both flow
through it with no new code path — the chain's determinism is inherited rather than re-implemented.

Two things stood in the way, and neither was visible from the plan:

1. **The graph's audio cords were mono.** Every producer/consumer audio port was declared
   1-channel and the interpreter copied `frames` samples from the producer's flat buffer into a
   mono input buffer, with a comment calling the per-port channel count "the seam" for a later
   stereo connection. The mixer's master output is *already* stereo (pan is equal-power into
   interleaved L,R), so patching the mixer into any second node would have handed that node the
   first *half* of the interleaved block — `L0, R0, L1, …` read as if they were consecutive mono
   samples. A mastering stage on the bus was impossible without stereo cords.
2. **An offline render carries the bus's latency at its head.** A lookahead limiter is latency by
   construction; a bounce that renders N frames from frame 0 would start with that much silence and
   end that much short. The engine had PDC to *align parallel paths* but no notion of the output's
   own transit for the offline path.

## Decision

**Stereo cords.** An audio input port now has a buffer of `channels * BLOCK` and a cord copies
`channels * frames` interleaved samples; fan-in stays a per-sample sum, so two stereo producers into
one stereo port add like with like. A cord carries **one** channel count: `Graph::connect` and
`Engine::validate_patch` both refuse a mismatch (the engine validates *before* the patch enters the
log, so a refused patch is never logged), and the mono path is bit-identical to before (a mono port
is `BLOCK` and the copy is the same flat sum). The out node's channel count already came from its
declared port, so the bus was already stereo end to end — only the *cord* was blind to it.

**A PDC bug, found while measuring.** The per-node PDC delay line runs over the node's interleaved
output buffer, so a delay declared in *frames* must be applied as `frames * channels` samples. It was
applied as `frames` samples, which silently halved every multichannel node's compensation — 240
samples is 240 frames of mono but only 120 frames of stereo. Nothing noticed because every node with
non-zero latency until now was mono (and the only latency-carrying node was a test double), so the
bug's first real trigger was the mixer in front of the master. Delay lines are allocated
`MAX_PDC * MAX_PDC_CHANNELS` so the frame cap keeps its meaning.

**Latency-aligned offline renders.** `Engine::render_with_drain_aligned` renders
`frames + flush_frames()` and drops the leading `flush_frames()` frames: the transit is *processing*,
not content, so the file starts at timeline frame 0 and the ballistics still see the piece's first
frames exactly as live playback from frame 0 would. `Graph::flush_frames()` (already the drain's
"how much is in flight") was the right quantity — but it read the **cached** `cum` that
`render_inner` writes each block, which is all zeros before the first block, so the transit is now
derived from a pure `cumulative_latency()` over nodes and cords. That also makes the *drain* correct
before a render. `HostSession::bounce` uses the aligned path; the engine-level tests measure the
transit as "the first non-zero output frame of a step" and assert it equals `flush_frames()` for both
a direct and a chained path (the equality the trim rests on).

**The `master` plugin** (`crates/engine/src/plugins/master.rs`), a stereo node with a stereo `audio`
in and out, mounted after the mixer and patched from it:

- **Compressor** — stereo-linked (`max(|L|, |R|)`, so a loud side cannot duck only itself and move
  the image), feed-forward, peak-detecting, with `threshold` (dBFS), `ratio`, `attack_ms`,
  `release_ms` and `makeup` (dB). Ballistics are exponential (`exp(-1/(ms·rate))`), recomputed when
  the times or the sample rate change. `ratio 1` is **exactly** transparent (the gain formula
  collapses to 0 dB), so mounting the chain for the limiter alone does not colour the mix.
- **Lookahead brickwall limiter** — `ceiling` (dBFS, default −0.4). A sliding-window **minimum** of
  the required gain (`ceiling/peak`, 1 when below) over a 5 ms lookahead is applied to the input
  delayed by that lookahead, so the gain is *already down* when a transient arrives: the ceiling is
  honoured **by construction** rather than by hoping the release is fast enough. The minimum is a
  monotonic deque over two preallocated rings (amortized O(1), no allocation, no per-block scan);
  attack is instant, release rises toward the window minimum and never past it. Tests pin the deque
  as a sliding minimum, the transparency case, the never-exceeds-ceiling case, and the reduction +
  recovery.
- **Metering** — per-side peak (post-limiter) and the block's gain reduction (dB, `0` =
  transparent) as atomics under the `master.meters` context key; the host exposes them as
  `HostSession::master_meters()` and `HostSession::snapshot().mastering`, so a shell reads the
  compressor's work as a value rather than computing it. The render path never blocks or allocates.
- **The bus** — mounting captures the previous out node and claims the output; unmounting removes
  the node and **restores the previous owner**, so dropping the mastering stage does not leave the
  graph silent. One documented truth: the fader is the mixer's `master.gain`; `makeup` belongs to the
  compressor. Two gains on the same bus would fight.
- **The wiring is the existing vocabulary**: `mount master @0`, then
  `patch mixer.audio master.audio @0` (a stereo cord, validated), then
  `set_param master threshold -24 @0`. No new command, no new code path — the same graph the live
  pump renders.

## Alternatives considered

- **Put the compressor inside the mixer** (params `master.threshold`, … on the mixer node). Rejected:
  it is the wrong shape for the platform (an effect rack on the bus is a plugin concern, and the
  plugin system exists precisely so effects are not welded into the mixer), it would make the mixer's
  param namespace grow without bound, and item 12 of the reviewed plan names a `master` *plugin*.
  The cost of doing it properly was the stereo-cord seam, which the code had already identified.
- **Mono-sum the bus into the master** (keep cords mono). Rejected: it throws away the pan the mixer
  already computes, and it would make the master a downgrade of the mix rather than a stage on it.
- **Resample/duplicate a stereo cord into a mono port** (or vice versa) instead of refusing it.
  Rejected: an automatic downmix or upmix is a mixing decision taken silently by the patch bay. A
  mismatch is a mistake, and the refusal names both counts.
- **Compute the head trim as `cum[out_latency]`** (the out node's cumulative latency). Rejected after
  measurement: the transit is the *sum* of the path's PDC delays and the nodes' own latency, not the
  out node's `cum` (they differ by the PDC delays added upstream to align parallel paths). The tests
  now measure the transit and assert it equals `flush_frames()`.
- **Trim by a constant (`MAX_PDC`, or the declared bus latency)**. Rejected: it would be wrong for
  any chain whose transit differs, and it would silently cut real audio when the bus shrinks.
- **A limiter with attack/release smoothing and no lookahead window** (gain computed from the current
  sample, smoothed). Rejected: it cannot be brickwall — the transient is already through by the time
  the gain moves — and "never a clipped file" is the point of the slice.
- **A per-sample gain envelope from the window (`min` then smooth upward with the release)**.
  Rejected the first draft: smoothing upward *above* the window minimum would re-open the ceiling on
  the frame that is leaving the window. The shipped order is minimum-first, then release clamped to
  the minimum.
- **Normalize the limiter's gain by the window sum / add a soft knee.** Rejected for alpha: neither
  changes whether the ceiling holds, and both add parameters to explain. The compressor's makeup is
  the level control.
- **Persist meter values in the log.** Rejected: meters are *events* (the mixer's already are). They
  are read from shared atomics; nothing about them belongs in the document.
- **Make the aligned render the default for `bounce` too.** It *is* now (bounce is the aligned path);
  `export` (item 13, next) is the sibling that adds f32/s16, the whole-arrangement frame count and
  the clipping refusal.

## Consequences

- **A mastering chain exists end to end**: `mount master` + `patch mixer.audio master.audio` +
  `set_param master …` gives a compressor and a brickwall limiter on the mix bus, in the graph, so
  the live pump and the bounce render the same DSP with the same determinism.
- **The bus is stereo and the cords know it.** The next stereo consumer (stereo clips, a meter bus, a
  second effect) needs no engine work; a mismatched patch is a named refusal.
- **Offline renders are latency-aligned**, which is what makes any lookahead effect usable in a
  bounce at all. The `bounce` file for a session with a master is the piece (plus its drain tails),
  not the piece plus 5 ms of silence.
- **The PDC fix is a behaviour change for any multichannel node with declared latency** — there were
  none in a real profile before this slice (only a mono test double), so no existing session changes;
  the new test measures the transit rather than trusting the bookkeeping.
- **Metering**: gain reduction is "the last rendered block", so an offline bounce that ends in a
  drained silence reports 0 dB. That is honest (a meter is not a peak-hold history) and the host test
  asserts the *effect* (RMS reduction) rather than the meter after a drain. A hold/peak-hold meter is
  a shell concern.
- **Still open**: `export` (item 13) — whole-arrangement frame count, f32 default, fixed-seed dither
  for s16, peak/RMS reporting, refuse-rather-than-clip; a shell display for the mastering meters (the
  values are published; the TUI shows the mixer's only); a soft knee, a sidechain filter and a
  separate limiter release; per-channel (unlinked) compression; and the compressor's `makeup`
  currently applies before the limiter (correct order — makeup cannot push past the ceiling).
- **Still open** (the gate's notes, recorded rather than hidden): a mid-stream sample-rate change does
  not recompute the lookahead/window (unreachable from the host — a session's rate is fixed at
  creation); there is no denormal flush-to-zero in the envelope/gain decay (performance only); and a
  flaky pre-existing capture test (`a_take_records_into_the_pool_and_plays`, "capture stopped with a
  partial frame") is unrelated to this slice but worth its own look.

## The gate's findings

The slice's gate returned `merge with changes`, having compiled two out-of-repo probes against the
built rlib and driven the real host binary ([review +
disposition](../../../../research/architecture/2026-09-24-alpha-slice-gate-f1-glm-standin.md)). All
three must-fix and all three should-fix findings were real and are fixed, with tests:

1. **A bounce under-trimmed after a mid-session `mount master`** (must-fix) — the transit was measured
   before the scheduled mount was materialized (`wire_pending` skips `flush_scheduled` when it has no
   player cords to lay, `wire_arranger` only flushes when the arrangement is dirty), so the trim came
   out zero and a full transit of silence was inserted at the head. Fixed by flushing the scheduler at
   the top of `render_with_drain`; re-verified with a before/after impulse run (file frame 5 220 → the
   exact 4 500).
2. **The limiter was not brickwall across a mid-stream `ceiling` change** (must-fix) — the window held
   gains derived under the old ceiling, which aged out over one lookahead (the gate measured 7.9× over
   the new ceiling). Fixed by windowing **peaks** and deriving the gain at read time, so a ceiling
   change covers the frames already in the delay line.
3. **A leftover debug env check on the render path** (must-fix) — `std::env::var` per block allocates
   and takes the environment lock, against the render-path invariant. Removed.
4. **The reduction meter compared misaligned frames** (should-fix) — it reported a phantom 114 dB while
   the limiter was idle, counting the delay line's warm-up. Fixed by delaying the dry input alongside
   the wet one, so the meter compares the frame that produced the output (and reports the *total*
   compressor + limiter reduction).
5. **PDC delay-line capacity could overflow** (should-fix) — a path whose cumulative latency exceeds
   `MAX_PDC` behind a stereo node asked for more than the ring holds (debug panic reproduced). Fixed by
   clamping the cumulative latency (and so the applied delay) at `MAX_PDC`.
6. **The mid-render bus-width guard matched `"mixer"` by name** (should-fix) — an engine-level
   scheduled `master` mount could swap the bus owner mid-call under an already-computed trim. Fixed
   structurally: park an unmount of the current bus owner, and a mount whose declared audio-out width
   differs from the bus.
7. **Non-finite input** (note, fixed anyway) — a NaN passed through, and an `+inf` became `inf * 0 =
   NaN` one lookahead later. Input samples that are not finite are now silenced at the bus.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-24.
