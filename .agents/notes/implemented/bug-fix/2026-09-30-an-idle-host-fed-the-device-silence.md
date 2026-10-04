# Agent Note: the output ring was filled only while playing, so an idle host starved the device

Status: implemented

## Problem

The iced shell's status line showed an ever-growing underrun count — tens of millions over a session —
and the owner's report that it ticked up when the sliders moved made it look like a UI or mixer
problem. It broke nothing: playback was fine throughout, and the count was only visible because the
shell prints it.

The cause is in the host pump, and it is one line of intent:

```rust
// crates/host/src/live.rs, fill_audio
while session.is_playing() && ring.len() < target { … }
```

The output ring is filled **only while the transport is playing**. But the device callback starts the
moment the stream opens — before any `play` — and [`fill_output`](../../../../crates/media/src/devices.rs)
counts an underrun for every frame it cannot pop. So a host sitting open at a stopped transport has a
running callback reading an empty ring, and accrues **one underrun per source frame**, in silence,
indefinitely.

Measured before the fix, on this machine (48 kHz, stereo):

| idle before `play` | underruns |
|---|---|
| 300 ms | 15,360 |
| 1,200 ms | 59,392 |
| 2,400 ms | 116,736 |

Exactly ~48,000 per second — one per frame — and **all of it accrued before any playback**. Slider
movement was incidental: the number was simply climbing on the clock, and the sliders happened to be
what the owner was doing while watching it.

## Decision

**Keep the ring fed while stopped, with silence.**

```rust
/// Keep the ring topped with **silence** while the transport is stopped.
fn fill_idle(ring: &Spsc<f32>, drops: &AtomicU64) { … }
```

called every pump iteration when the transport is not playing. The device then always has frames to
pop, so its callback stops counting starvation, and `underruns` means what its name says: a real
shortfall. After the fix the same three idle windows measure **0, 0, 0**, and playback measures 0 too.

Pinned by `live::tests::an_idle_host_keeps_the_output_ring_fed`, which fills an idle ring and asserts
it reaches the target, contains a whole number of stereo frames, counts no drops, and is a no-op when
called twice.

## Alternatives considered

- **Prime the ring before `play()` instead.** Rejected — and it was the first thing tried, on the theory
  that the device resumed onto an empty ring. The ring was measured **completely full** at the moment of
  play and the burst was unchanged. The starvation is not at the play crossing; it is the whole idle
  period. The fix has to run continuously, not once.
- **Request a smaller device period (`BufferSize::Fixed`).** Rejected for this bug: it is likely the
  right configuration for latency, but it does not touch the starvation — the callback is 2048 frames
  and the burst is unchanged by the period. (Changed, measured, and **reverted**, so the tree does not
  carry an unrelated behaviour change.)
- **Stop pausing the device.** Rejected: measured, no change to the count, so pause was never the cause.
- **Only count underruns while playing.** Rejected: it makes the number pass by definition rather than
  fixing the starvation. Feeding silence is a real fix and costs a memory write of silence in a loop
  that runs a few times per second at most.
- **Filter the count at the shell.** Rejected: the counter is the host's contract; a shell hiding it
  would leave the next shell to rediscover it.

## Consequences

- **The counter is now trustworthy**, which is what makes it useful: a non-zero `underruns` during
  playback is a genuine shortfall rather than the cost of standing still. The `--sweep` instrument in
  the iced spike is the regression guard, and it keeps **both** probes the diagnosis needed: it
  reports the idle rate over a second, and it re-runs the idle → fader → play crossing at three
  pre-play delays (300/1200/2400 ms) so a regression cannot hide behind a single repeated sample. The
  delay sweep was dropped by the record slice and restored at the merge gate; a note that owns a fix
  has to keep naming the instrument that guards it.
- **Cost is a little wasted work while idle** — silence is pushed in a loop each pump iteration. The
  ring is small (16,384 samples) and the write is a handful of operations when already full, so this is
  accepted rather than optimised. It also keeps the device genuinely running, which is what a DAW wants
  on `play`.
- **The instrument was wrong before the fix was right, twice.** My first readings said "idle rate 0/s"
  and "flat while stopped", both of which pointed away from the true cause: I sampled the snapshot
  **before the device thread had opened the stream**, so `0 → 0` looked like a quiet device when it was
  an unopened one. The measurement that settled it varied the idle delay — the burst scaled with it at
  exactly 48,000/s. Two false negatives from the same instrument is the argument for varying a
  parameter rather than repeating a sample.
- **The owner's observation carried the diagnosis.** "No increase after the first play" is true, and it
  is only true because the ring is full by then; the same sentence read against my own bad measurement
  is what forced the delay-scaling test that found it.

Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-30.
