# Agent Note: A take's two clocks are rates, so neither may be zero

Status: implemented

## Problem

`media::Capture::start` validates two of its six arguments — `channels` against
`CAPTURE_CHANNELS_SANITY` and `take_id` against the filename grammar — and then trusts the two
clocks. The demux thread turns them into a drift ratio and sizes a buffer from its reciprocal
(`crates/media/src/capture.rs`):

```rust
let ratio = input_rate as f64 / sample_rate as f64;
let session_cap = (n_frames as f64 / ratio).ceil() as usize + 2;
```

With `input_rate == 0` the ratio is `0.0`, so the quotient is `+inf`; `inf as usize` saturates to
`usize::MAX`, and `usize::MAX + 2` overflows. In a debug build — every `cargo test`, every
developer run — that is a **panic on the demux thread**, which `Capture::stop` reports as the
misleading `capture demux thread panicked`, naming a thread fault where the cause is an argument.
In release the add wraps, `session_cap` is 1, and `DriftCompensator::pull_output` writes one
sample per batch with `pos` frozen at 0 (`drift.rs:64`), so `consumed` is 0 (`drift.rs:70`),
`pending` is never drained, and every batch appends 512 samples it never gives back: a take that
records a constant DC value while its buffer grows without bound for the length of the recording.

Nothing in-tree reaches it — every call site passes a real rate (`hardware_input.rs:29`,
`p1_2.rs:88/258/295/353`, `host/src/lib.rs:930-937`) and `DriftCompensator`'s own tests use
48 001/47 999 — but `start` is public and the missing guard is an inconsistency, not a decision:
the sibling APIs that take rates refuse zero (`Resampler::new`, `resample.rs:144`;
`Pool::write_source`, `pool.rs:321`; `Pool::conform`, `pool.rs:555`), and `Host::start_recording`
refuses a zero *session* rate (`host/src/lib.rs:917`) while passing `input_rate` straight
through. The asymmetry is the tell.

Reported by the [Space Bunny review
2](../../../../research/architecture/2026-09-29-space-bunny-review-2-media-io.md) as MAJOR #3 and
confirmed independently: same class as `4-host#5` (a take's channel count aborting the process),
which was fixed by a bound at the point that sizes from the number. This is that point.

## Decision

**`Capture::start` refuses a zero `input_rate` or `sample_rate`, next to the channel-count check,
naming the parameter and the offending pair:**

```
capture rates must be non-zero (got input_rate 0 → sample_rate 48000): the drift ratio is
input_rate / sample_rate
```

The check sits **before** `create_dir_all` and before the `ChannelWriter`s are opened, so a
refused take leaves no pool directory, no WAV header and no demux thread behind — the same
"a refusal has no side effects" rule the [take-declaration
note](./2026-09-29-a-take-declaration-is-bounded-before-it-is-sized.md) states for the command
arm. The doc comment now says both rates must be non-zero, which is the contract `Resampler::new`
already carried in prose.

`DriftCompensator::new` is left alone: it is an infallible constructor whose only production caller
is this one, and with both rates non-zero its ratio is finite and positive by construction, so a
second guard would be a panic in a place the crate no longer needs one.

## Alternatives considered

- **Assert in `DriftCompensator::new` as well** (`ratio.is_finite() && ratio > 0.0`). Rejected: the
  review offers it as closing the hole "at the source", but the source is now closed by an
  `Err` on the path that can actually reach it, and a `debug_assert!` here would be a panic in a
  public constructor whose failure mode is already unreachable. If a second caller ever appears,
  the guard belongs at *that* caller's boundary, not duplicated underneath this one.
- **Also refuse `input_rate == 0` in `Host::start_recording`.** Rejected: `start_recording`
  propagates the capture's message verbatim with `?`, and the device path passes
  `handle.sample_rate`, so the guard would only re-word a message that already names the
  parameter. The session-rate check that *is* there is not moved either — it keeps a session that
  never started from reaching a take at all.
- **Clamp a zero rate to the other one** (treat a missing device clock as a passthrough). Rejected:
  a zero is a caller bug, and silently recording at the wrong clock produces a take that is the
  wrong length and sounds right — the failure this crate refuses everywhere else.
- **Let the demux thread detect it and record the error into `err`.** Rejected: the diagnostic
  arrives only at `stop`, after the thread has already burned the take's memory and the message
  names the wrong place. A refusal belongs before the thread exists.

## Consequences

- `Capture::start` has one more refusal, and it is a `Result` error on a user-reachable path — no
  panic, and nothing the platform can legitimately produce is refused: both clocks come from a
  session rate and a device rate, neither of which is zero for any opened device.
- Regression test, `capture::tests::a_zero_rate_is_refused_by_start`: `start` with
  `input_rate = 0` and with `sample_rate = 0`, asserting each message names its own parameter and
  that the pool directory was never created. On the pre-fix logic it fails on the first `start`
  (it returns `Ok`), which on the old code also leaves a take whose demux thread panics.
- No existing test moves: `capture_writes_per_channel_pool_sources`,
  `a_tail_that_never_completed_a_frame_is_padded_not_dropped`,
  `faster_drift_records_into_session_frames` and `slower_drift_records_into_session_frames` pass
  real rates on both clocks, and `p1_2.rs` / `hardware_input.rs` (the latter `#[ignore]`d
  hardware) pass `rate, rate`.
- The drift compensator's contract is unchanged and still stated in `drift.rs`: a caller feeds
  device frames and pulls session frames. The zero-rate arithmetic it could not survive is now
  refused one layer up.
- The [Spike B note](../architecture/2026-08-17-spike-b-media-engine.md)'s decision to keep the
  compensator allocating (`Vec` buffering is fine off the audio path) is unchanged; the demux thread
  still owns its buffers and is still not the render path.
- Tests: `cargo test -p media --lib` green (127 passed, 3 pre-existing hardware/soak ignores),
  `cargo test -p media --test p1_2` green (4 passed, 1 hardware ignore).

*Authored with Space Bunny · OpenCode, 2026-09-29.*
