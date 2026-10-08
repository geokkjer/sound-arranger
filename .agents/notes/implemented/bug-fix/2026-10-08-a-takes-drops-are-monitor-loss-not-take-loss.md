# Agent Note: a take's `dropped` counter is monitor loss, not take loss

Status: implemented

## Problem

The iced recorder spike showed a take whose `{n} dropped` climbed steadily while recording —
`0` for the first 1.25 s, then `14 336`, `38 912`, `62 464` a quarter-second apart, and
climbing at roughly 48 000 per second — right beside a frame count that kept growing. On an
8.9 s take it reads as ~360 000 dropped frames. The owner reported it as "a lot of underruns
when play and record at the same time".

Nothing was lost. Three measurements fixed the diagnosis (one iced shell, one Scarlett 2i2,
48 kHz):

- **Output device underruns: 0.** `--sweep` (play only) and a new `--record-underrun`
  (play + record) both read `underruns 0` at open, idle, during play, through two seconds of
  recording, and after stop. The known transport-transition burst did not reproduce.
- **The take was whole.** The take the probe stopped reported 96 768 frames; its pool source
  (`take-18.ch0.wav`) is exactly 96 768 frames, and `ch1` matches.
- **The climbing number was the monitoring ring.** `Capture::start` allocates one
  `Spsc<f32>` of 65 536 frames per channel for `CaptureNode` monitoring
  (`crates/media/src/capture`), and the demux thread pushed **every** sample into it
  unconditionally. 65 536 frames is 1.365 s at 48 kHz: the ring filled at 1.365 s and every
  later sample was counted as `dropped`. Nothing in the host or either shell mounts a
  `CaptureNode` — `channel_ring` is consumed only by `media`'s own tests — so the ring could
  never drain.

The counter was therefore measuring monitor quality while being displayed, and documented, as
material loss. The host's `TakeReport` carried `/// Source frames the capture had to drop (the
ring was full)`, and both spikes printed it next to the take as `N dropped`. A misleading
number beside a take is worse than no number: it trains the reader to ignore the one field
that would report real starvation.

## Decision

**A monitoring ring is filled only while a monitor is attached, and the counter is named for
what it counts.**

- `Capture` keeps one `monitored: Vec<Arc<AtomicBool>>`, set by `channel_ring(k)` — asking for
  a ring *is* attaching a monitor. The demux pushes into a channel's ring only when that flag
  is set, so an unmonitored take does no ring work and counts no drops.
- The host API says so: `TakeReport::dropped` and `RecordingStatus::dropped` become
  `monitor_dropped`, documented as best-effort monitor loss, zero while nothing monitors.
- The shells label it `monitor drops` and show it **only when non-zero**, so an ordinary take
  reports frames and nothing else.
- The **persisted** `take <id> <frames> <dropped> <channels> <at_frame>` line keeps its
  `dropped` token for `host v1` compatibility; only its documentation changes. It was always
  this counter, so no reader was ever misled about the audio — the WAV is authoritative and
  the declaration is provenance.

The instrument that found it stays: `spikes/iced-shell --record-underrun` samples the output
underrun counter and monitor drops across a play + record cycle, printing the delta between
samples so a one-time transition burst and a climbing counter cannot be confused.

## Alternatives considered

- **Relabel in the shells only.** Rejected: the field name and its doc comment stay wrong, and
  the next reader re-derives the same false conclusion from the API.
- **Keep filling the ring, and stop counting once it is full.** Rejected: it hides a real
  monitor loss behind a full ring's silence, and it leaves the ring doing 48 000 pointless
  pushes per second for a monitor that does not exist.
- **Keep pushing but drop silently when the ring is full.** Rejected: a monitor would miss
  samples with no signal at all — the counter exists precisely so a monitor consumer can see
  what it lost.
- **Remove the monitoring rings from the capture path.** Rejected: they are the live
  monitoring seam (`CaptureNode` → mixer), exercised by `capture_into_adaptable_mixer_and_pool`,
  and the iced spike's meters are the evaluation's point.
- **Reset the counter when a take starts.** Rejected as a fix: the counter was never
  accumulating across takes, so this changes nothing and would hide the mechanism.

## Consequences

- `dropped` now means "monitor samples this take lost", is zero in every run that does not
  monitor, and can no longer be read as take loss. `Take.attempted`-style fidelity reporting
  for real loss (source-ring overruns, worker misses) remains a separate counter — the three
  must not be conflated.
- Sessions saved before this change carry a large, inflated `dropped` value in their `take`
  lines. It is informational only and is not read back into audio; a load keeps it as
  provenance.
- The regression test
  (`crates/media/src/tests/capture.rs::monitor_drops_are_counted_only_while_a_monitor_is_attached`)
  feeds three ring capacities with no monitor and asserts `dropped == 0` and the take whole,
  then attaches a monitor that never drains and asserts drops are counted.
- Attachment is one-way and from the point of attach: `channel_ring` marks the channel
  monitored, so a consumer mounted after a take began sees the take from the mount forward.
  That is the monitoring path's existing best-effort contract, now stated.

*Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-10-08. Found while checking the
recorder iced spike with the owner: the take was intact, the counter was not.*
