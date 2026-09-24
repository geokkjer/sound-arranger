# Reviewer gate (stand-in) — alpha slice F1 (the master bus), GLM-5.3

> Research input, 2026-09-24. Co-worker: **GLM-5.3** (`zai/glm-5.3`), read-only, standing in for the
> designated gate **Kimi K3** (unusable: every API run died with no output — see
> [`2026-09-23-kimi-gate-alpha-slices.md`](2026-09-23-kimi-gate-alpha-slices.md)).
> Scope: the master-bus slice — stereo audio cords, a PDC per-frame fix, latency-aligned offline
> renders, and the `master` compressor + lookahead-limiter plugin. The reviewer compiled two
> out-of-repo probes against the built engine rlib and drove the real host binary with `host v1`
> scripts (`/tmp/probe_dsp.rs`, `/tmp/probe2.rs`, `/tmp/probe_script.txt`), so every finding carries
> executed evidence.
>
> **Disposition: `merge with changes`; all three must-fix and all three should-fix findings were real
> and are fixed, with tests.** Two of them were things the slice's own tests could not see (a
> mid-session mount path the in-tree test never takes, and a parameter change mid-stream), which is
> exactly what the gate is for.

## Must-fix

1. **A bounce under-trims after a mid-session `mount master`.** `HostSession::render_with_drain`
   measured the graph's transit *before* the scheduled mount was materialized: `wire_pending`
   early-returns without `flush_scheduled` when no player cords are pending, and `wire_arranger`
   returns early when the arrangement is not dirty. Reproduced by the reviewer: a bounce after
   arrange → bounce → `mount master` → patch → `set_param` → bounce put ≥712 frames of exact zeros
   at the head of the file and shifted the content 720 frames late (the file length looked right,
   which is why the in-tree test — whose `Arrange` marks dirty — missed it).
   **Fixed** by flushing the scheduler at the top of `render_with_drain`, before anything is
   measured. Re-verified here with an impulse fixture and a before/after run: the spike sits at file
   frame **5 220** without the flush and **4 500** with it (the exact global offset, `6000 - 1500`).
2. **The limiter was not brickwall across a mid-stream `ceiling` change.** The window stored the
   *required gain* derived at push time, so after the ceiling dropped, the stale (too high) values
   aged out over one lookahead; the reviewer measured the output at 0.5 against a 0.0631 ceiling
   (7.9×). **Fixed** by windowing the **peaks** and deriving the required gain at *read* time, so a
   ceiling change applies to every frame already inside the delay line. Regression test
   `a_lower_ceiling_applies_to_the_frames_already_in_the_delay_line` (the declared range caps a
   direct `set_param` at −12 dB, so the overshoot the test guards is 2× the achievable ceiling).
3. **A `MASTER_DEBUG` env check on the render path** — `std::env::var` per block allocates a `String`
   and takes the environment lock, violating the "render never allocates, never blocks" invariant
   (a leftover from building the slice). **Removed.**

## Should-fix

4. **The gain-reduction meter compared misaligned frames.** It divided the *current* dry input by the
   *delayed* output, so a freshly (re)mounted limiter reported a phantom **114 dB** of reduction
   while idle — the delay line's warm-up counted as gain reduction. **Fixed**: the dry input is
   delayed alongside the wet path, so the meter compares the frame that actually produced the output
   (and the *total* compressor + limiter reduction is what it reports). The compressor test now reads
   the meter while the loud section plays — a meter is "the last rendered block", so an offline bounce
   that ends in drained silence reports 0 dB, correctly.
5. **PDC delay-line capacity could overflow with per-node-legal latencies.** `set_delay(pdc *
   channels)` can exceed the `MAX_PDC * MAX_PDC_CHANNELS` allocation when the *path's* cumulative
   latency exceeds `MAX_PDC` behind a stereo node (three ~2731-frame stereo nodes suffice; the
   reviewer reproduced the debug panic). **Fixed** by clamping the cumulative latency — and therefore
   the applied delay — at `MAX_PDC` in both the pure plan and `render_inner`, so the frame cap keeps
   its meaning and every line fits. Regression test
   `a_chain_beyond_max_pdc_saturates_instead_of_panicking`.
6. **The mid-render bus-width guard matched `"mixer"` by name**, so an engine-level (non-host)
   scheduled `master` mount applied mid-call and changed the bus owner and latency profile under a
   `render_with_drain_aligned` trim that had already been computed. **Fixed structurally**: the guard
   now parks an unmount of the *current bus owner* and a mount whose declared audio-out **width**
   differs from the bus, with no plugin names. (`unmount_is_sample_accurate` in `spikes_a` still
   passes: unmounting a non-bus plugin stays sample-accurate.)

## Notes (recorded, not code)

7. **Non-finite input on the bus** — a NaN passed through as one sample; an `+inf` made the required
   gain `ceiling/inf = 0` and turned into `inf * 0 = NaN` one lookahead later. **Fixed anyway** (it is
   four lines at the point where upstream garbage converges): input samples that are not finite become
   silence, with `non_finite_input_is_silenced_not_propagated` pinning the recovery.
8. **A flaky pre-existing capture test** (`a_take_records_into_the_pool_and_plays`: "capture stopped
   with a partial frame (1/2 samples) dropped", failed twice for the reviewer, passes in isolation,
   no capture code in this slice). Recorded as debt, not hidden.
9. **A mid-stream sample-rate change** would not recompute the lookahead/window (the ballistics are);
   unreachable from the host (a session's rate is fixed at creation and a change is refused).
10. **No denormal flush-to-zero** in the envelope/gain decay — performance only, and the engine has no
    FTZ policy anywhere.

## Checked and correct (executed, not assumed)

- **`MaxWindow`** (formerly the min window) index algebra: eviction boundaries, ties, ring wraparound
  (`cap + 1` slots, one always free), and the window covering exactly `[j, j + lookahead]` for the
  frame leaving the delay line. A random-noise run at a −3 dBFS ceiling overshoots by exactly ×1.0000.
- **First-block brickwall** (a constant 1.4 from frame 0 comes out at exactly the ceiling) and
  **lookahead = 1** (window cap 2, delay 1 frame) hold by the same algebra.
- **`ratio 1` is exactly transparent** — maximum sample error 0.0.
- **Compressor**: stereo link (one gain from `max(|L|,|R|)`), `exp(-1/(ms·sr))` coefficients recomputed
  on a rate change, correct over-threshold slope.
- **`flush_frames()` equals the measured transit** for chained stereo paths (the in-tree test), and the
  pure `cumulative_latency()` matches `render_inner`'s per-block copy.
- **Mono bit-identity**: the copy loop and the delay arithmetic are unchanged for 1 channel; growing
  the ring capacity changes no numbers.
- **Channel-mismatch refusal in both places**, the engine's *before* anything is logged; interleaved
  fan-in sums like with like; input buffers are sized and cleared to the exact `channels * frames`.
- **Integration**: mount / patch / `set_param` / unmount through the host text format work and replay;
  no parameter-name collisions (the mixer owns `master.gain`); unmount restores the previous bus owner
  and removes `master.meters`; the snapshot's `mastering` is gated on the meters' presence.
- **Determinism**: a master session bounces byte-identically across a rewind; independently, two runs
  of a script and the *saved session's own replay* bounce to the same md5.
