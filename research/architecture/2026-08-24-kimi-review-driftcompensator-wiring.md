# kimi review — DriftCompensator wiring into the Capture (2026-08-24)

Session: `session_d9864022-f80c-458f-abb8-26c7e41a3030`

kimi reviewed the wiring of the (previously dead) `DriftCompensator` into the capture demux.
Verdict: the shape is right (per-channel compensators in the demux thread, pool WAV at the
session rate, gross mismatch refused at playback), but there was a **real correctness hole
in one drift direction**.

## Must-fix (integrated)

- **`ratio < 1` (device slower than session) was structurally disabled.** The `n_frames`-sized
  session buffer capped `pull_output`, so the take drifted at `input_rate` and `pending` grew
  unbounded. **Resolved:** the buffer is sized `ceil(n_frames / ratio) + 2`, covering both
  directions; `frames()` now accumulates the *session* frames actually pulled (`last_ok`), not
  the device frames. Added the **mirrored `slower_drift` test** (47999→48000) that exposed it.
- **`pull_output`'s `drain(..floor(pos))` could panic** on a short (take-tail) batch when a
  fractional `pos` carried over: `floor(pos)` exceeded `pending.len()`. **Resolved:** clamp
  `consumed` to `pending.len()`.

## Should-fix / worth-considering (noted or deferred)

- **Playback rate-mismatch refusal vs capture drift are separate.** `ArrangerNode::new`
  refuses a *source WAV* at a different rate (a play-back of an imported file — gross mismatch
  needs a real resampler, not drift linear-interp). The capture produces *session-rate* WAVs
  (declared at `sample_rate`), so a recorded take is never refused; the DriftCompensator
  corrects the tiny *device-clock* drift. No tolerance band is needed between the two.
- **`passthrough` (input==output) still runs the interpolator at ratio 1** — bit-exact
  (`frac == 0.0`), the round-trip test passes legitimately; an explicit bypass is a trivial
  future optimization.
- **Static nominal ratio** — corrects the constant nominal-rate offset; adaptive (buffer-level
  feedback) drift tracking is deferred.
- **Tail flush on stop** — residual compensator `pending` is dropped at stop (a sub-batch
  boundary artifact); clamping prevents the panic but a true tail drain is a follow-up.
- **Production caller device-rate plumbing** — all 6 callers pass `input_rate == session_rate`
  (passthrough); the real audio-device caller passing the actual stream rate is a follow-up
  (no production capture caller yet beyond the reference host/hardware test).
- **Test timing** — a fixed 100 ms sleep + feed loop; poll-with-deadline is a robustness
  improvement (matches the existing capture tests).

The full critique is the reviewer's response in this session.
