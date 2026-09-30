# Agent Note: review-driven hardening — WAV size guard, player-retire park, rate check, render Result

Status: implemented

## Problem

The full-codebase review (GLM-5.3) verified four more criticals/majors at untested edges.
This change addresses #3 (a >4 GiB take silently truncates to a corrupt WAV header),
#4 (a retired/finished clip frees its ring on the render thread), #5 (a rate-mismatched take
plays pitch-shifted with no diagnostic), and #6 (`render()` panicked on a data-driven error).

## Decision

- **`wav.rs`** — a `data_bytes(frames, channels, bytes_per_sample) -> Result<u64>` helper does
  `checked_mul` and refuses `> u32::MAX - 37` (the RIFF size field is `36 + data_bytes (+1
  pad if odd)`; `-37` covers the pad conservatively). Used by both `patch_sizes` (finalize)
  and `recover`. A take that big fails loud instead of writing a corrupt small header. The third
  parameter is a sample **width**, not a format flag — the
  [recovery-width bug-fix note](../bug-fix/2026-09-29-recovery-uses-the-headers-sample-width.md)
  records why a float/16-bit guess truncated a recovered 24-bit take.
- **`stream.rs`** — the reader thread now **parks at EOF** (sleeps while holding its Arc
  clones) until the player's `Drop` sets `stop` — so a retired player's ring frees on the
  reader's own exit, off the audio thread (before, `self.cur = Some(finished.incoming)`
  dropped the player on the render thread). Verified `Drop` **detaches** (no join), so the
  retire path does not block the render thread.
- **`arranger.rs`** — `ArrangerNode::new` takes a `session_rate` and refuses any clip whose
  source `WavReader::sample_rate()` differs (named error, "wire DriftCompensator or resample"
  — `DriftCompensator` exists). A rate-mismatched take can no longer play pitch-shifted
  silently.
- **`host/lib.rs` + `main.rs`** — `render()` returns `Result<Vec<f32>, String>` (wire ops
  `?`-propagated; `run_script` propagates); the `Unmount` mixer arm clears the mixer-dependent
  host state (`pending_cords`, player mailboxes/counters, `mixer_channels`) so a later command
  can't reach a stale wiring path; the host panic sweep turned the last `.expect` into a clean
  `Err` and the CLI stdin read into an exit-2 error.

## Alternatives considered

- **`-36` bound** — rejected (kimi): the RIFF size counts an odd-data pad; `-37` is the safe
  conservative bound. Our writers always produce even `data_bytes`, but the guard is strict.
- **Clamp-and-recover instead of refusing a > u32 take** — rejected for now (RF64 is the real
  answer; refusing is honest).
- **Keep `data_bytes`'s parameter a format flag** — rejected by the 2026-09-29 review fix above: a
  flag cannot express 24-bit, so recovery described such a take at 2 bytes a sample.
- **Disposal queue for the retire free** — the fully-uniform fix (covers the mid-read race
  too) but allocates on the render path (a no-alloc tension); deferred as a dedicated look.
- **`render()` keeps `.expect`** — rejected (kimi): a data-driven error (unmount-mixer-after-
  play) panicked a host; `Result` is clean.

## Consequences

- A >4 GiB take refuses loudly (the pool records long jams — ~6.2 h mono float @48 kHz crosses
  4 GiB); the boundary is tested.
- A retired/finished clip's ring frees off the render thread; the reported EOF case is fixed.
  (The mid-read retire race is a documented follow-up; `Drop` detaches so no join-block.)
- A rate-mismatched source refuses at construction (a 44.1 k take in a 48 k session).
- The host never panics on a data-driven script/wiring error — `render()` propagates.
- 3 new tests (data_bytes boundary, rate-mismatch refusal, plus the earlier per-frame
  devices + parse_script suites). 18 workspace suites green, clippy clean.
- **Deferred**: the mid-read retire race (disposal queue — no-alloc tension); the WAV pad
  encountered only for odd data_bytes (not reachable by our even writers); the in-flight
  player across a mixer unmount→remount; RF64.
