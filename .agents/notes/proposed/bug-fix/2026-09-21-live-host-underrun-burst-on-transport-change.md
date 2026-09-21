# Agent Note: the live host's underrun counter jumps ~2–3 s on the first transport command

Status: proposed

## Problem

Both shell spikes display the live audio state, including `underruns` from
[`media::devices`](../../../../crates/media/src/devices.rs) — the count of device frames the output
callback had to fill with silence because the source ring was short. With a real output device open
(48 kHz · 2 ch), the counter behaves like this:

| what the shell did | `underruns` at quit |
|---|---|
| idle ~6 s, no transport command | **0** |
| one seek (`.`), transport stopped, wait 2 s | 95 744 |
| one seek, wait 10 s | 95 744 |
| two seeks, wait 10 s | 95 232 |
| play then immediately stop (`space`, `s`) | 143 360 |
| play ~3 s, never stopped | 143 872 |

Three properties, all measured:

1. **Idle is clean.** A stopped transport with no commands counts zero underruns.
2. **The first transport command produces a single jump** of roughly **95 k–144 k device frames
   (≈ 2–3 s at 48 kHz)**.
3. **It does not grow afterwards.** The count is identical 2 s and 10 s after the jump, and flat
   while playing — so it is a one-time burst at a stream *transition*, not sustained dropout. The
   size varies between runs.

The counter is in **device frames**, not events: `fill_output` adds one per starved frame, so a
single callback that runs with an empty ring adds its whole buffer length.

The part that does not add up: **a pure `TransportSeek` on a stopped transport produces the jump**,
and that path never calls `Stream::play()`. The host sends `TransportStop` then `TransportSeek`
(seek is "rebuild + render to target", O(target)); `TransportStop` calls
`OutputHandle::pause()` and drains the ring. Either the pause does not take effect promptly enough
to stop the callback draining a large empty buffer, or something in that path restarts the stream.
**The cause is undetermined** — this note records the measurement and the reproduction, not a
diagnosis.

Why it matters beyond a wrong number: **the counter cannot currently be used to detect real
dropout during playback**. A shell that shows it as a health signal tells the user the engine is
broken when nothing is wrong, and would equally hide a real starve in the noise floor of the
transition burst.

## Proposal

1. **Reproduce with device-level instrumentation** before changing anything: log
   `pause()`/`play()` calls, the ring length at each callback, and the callback buffer size, then
   identify which of these holds:
   - `pause()` returns before the stream actually stops (cpal/ALSA/PipeWire period latency);
   - the ring drain in the `TransportStop` handler races the callback;
   - the seek path re-opens or re-configures the stream.
2. **Then either fix it or rename the field.** The acceptance is that the number means one thing:
   - **fix** — a stopped transport counts zero across any command sequence, and a play/stop cycle
     counts zero when no frame was actually late; or
   - **scope it honestly** — split "starved while starting/stopping the stream" from "starved
     while playing", and show only the latter as a dropout alarm.
3. **Pin it with a test.** `fill_output`'s per-frame counting already has a unit test; the
   transition behaviour needs an `#[ignore]`d hardware test (this machine has a Scarlett 2i2) that
   asserts the published counter after a play/stop cycle and after a seek-while-stopped.
4. **Re-label the shells** once the semantics are settled — both spikes currently print the raw
   counter, which is how this was found.

## Alternatives considered

- **Leave it.** The counter is only surfaced in the spikes, and the engine's determinism tests
  (byte-identical bounces, `underruns == 0` on the arrangement path) are unaffected. Rejected: both
  shell spikes display it, so the first thing a user reads on a healthy system is "underruns
  143872", and the field stops being informative for its actual purpose.
- **Reset the counter on transport transitions (hide the symptom).** Cheapest change, and tempting —
  but it discards real information (a genuine starve during a transition is exactly what one wants
  to see) and leaves the underlying question unanswered. Rejected as a fix; acceptable only as part
  of option 2's split, where the two cases keep separate counters.
- **Make `pause()` block until the callback confirms silence.** A plausible fix if hypothesis 1
  holds, but doing it now would be guessing at the cause — and it touches the realtime-adjacent
  control path, where the repo's rule is to measure first.
- **Stop showing audio state in the spikes.** Rejected: the live-audio readout is a core part of
  what the shell evaluation is testing (the iced spike's whole point was "the device opens and the
  meters follow it").

## Acceptance criteria

- A device-level measurement identifies which hypothesis holds, recorded in this note (or its
  replacement).
- After the change: `underruns` is 0 for a stopped transport across play/stop (and seek), and
  during playback it counts only genuinely late frames — or the field is split and the shells show
  the dropout-only counter.
- The hardware test (`#[ignore]`d) asserts the post-cycle counter, and the media/host notes state
  the counter's units (device frames) and semantics.

## Risks

- **It may be benign.** If it is cpal/ALSA pause latency, the correct outcome is naming and
  documentation, not code — and the "fix" would be to stop calling it a dropout count.
- **Realtime-path risk.** Any change here touches the stream lifecycle next to the render path;
  the mitigations are that the counter is observational (no audio corruption is implied by the
  measurement — probes, bounces, and the byte-identical tests all still pass) and that the
  instrumentation lands before the fix.
- **Hardware-dependent.** Measured on one device (Scarlett 2i2 via the default cpal host) on one
  machine; the burst size varies run to run, so the test must assert a *property* (zero, or
  not-growing) rather than a magic number.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-21.
