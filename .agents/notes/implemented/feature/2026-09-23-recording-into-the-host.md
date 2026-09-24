# Agent Note: recording into the host — `record <take_id>`

Status: implemented

## Problem

The product's own description is *record long generative runs, then arrange them* — and the first half
did not exist. `HostCommand::Record` was a stub that returned
`Err("recording requires a device — … not wired into the host")` (`crates/host/src/lib.rs`), and
`media::Capture` was reached only by `media`'s own tests. So no shell could record: material entered the
pool by importing a file or by pointing `pool` at a directory, and the capture machinery that *did*
exist (device-clock drift compensation, per-channel writers, crash recovery, peaks) was unreachable from
the product. The alpha plan's co-work passes ranked this the single most important gap — above
save/open, because a session that cannot record has no material to save.

## Decision

**`record <take_id>` captures the default input device into the session's pool, and `record stop`
finalizes the take.**

- **A take is pool material, not an arrangement edit.** The takes are written by `media::Capture` as one
  mono float WAV per input channel (`{take_id}.ch{k}.wav`) plus a `.peaks` sidecar each, **at the session
  rate**: the device's own clock is drift-compensated into session frames, so the material is immediately
  arrangeable and never needs the pool's rate conformance. `Pool::list` sees them like any other source.
- **The command is an action, not state.** A recording session is not replayed by a log replay — a
  performance cannot be re-recorded from a command list — so `Record`/`RecordStop` are actions (like
  `Bounce`), and what the log records is the *clip* you make from the take. That is the same
  determinism boundary the media-engine note draws: the log is intent, the take is ground truth.
- **The device path is a thin shell over a testable seam.** `HostSession::start_recording(take_id,
  ring, input_rate, channels)` is the seam: the caller owns an interleaved ring (a device callback in
  production, a test fixture in the harness). `HostSession::record(take_id)` reads
  `media::devices::default_input_config()` for the device's channel count and rate — the take's channel
  count must match what the stream actually delivers — opens the input device with `open_input`, and
  refuses if the two calls disagree about the rate (the default device changed between them).
- **The `!Send` stream lives in the session.** A cpal `Stream` cannot cross threads, and the session
  never leaves the thread that built it, so `Recording` holds both the stream and the capture demux.
  `stop_recording` drops the stream **first** (stop feeding), then stops the capture (drain, finalize
  the WAV headers, write peaks) and reports `TakeReport { take_id, frames, dropped, channels, sources,
  sample_rate }`.
- **The shell sees it.** `HostOutcome` carries `recording` (a live indicator: take id, channels, frames,
  dropped) and `last_take` (the finished report, announced once by the shell). The TUI shows
  `● recording jam — 2 ch, 48000 frames, 0 dropped (… record stop ends it)` and, on completion, the take
  and its source ids with the next step named.
- **Recording is independent of the transport.** A take's frame 0 is *not* the playhead: the clock and
  the take are connected by the drift compensation to the session rate, not to the arrangement. Aligning
  a take to the grid is an explicit later step (the jam-layer note's fixed-offset problem), and recording
  while playing is allowed precisely because it makes no alignment promise.
- **The input ring carries the device's channels, interleaved** (not a mono ring). The mono-ring design
  was correct for a caller that only wanted channel 0, but the product records multi-channel material
  from a stereo/4-channel interface; the ring and the demux must agree, and `InputHandle::channels` is
  what makes them agree from one call.
- **Fixed-shape commands are arity-strict.** `exact()` now rejects extra words for `pool`, `record`,
  `save`, `load`, `session_rate`, `unmount`, `bounce`, `undo` and `redo`. This came out of writing the
  tests: `record bad id!` silently became a take called `bad` (the parser ignored the stray word) and
  **opened the real input device in a unit test**. A silently ignored word is a typo with consequences.

## Evidence

- `a_take_records_into_the_pool_and_plays` — the whole loop with the device replaced by the seam: start →
  feed a second of stereo tone → stop → the pool has `jam.ch0`/`jam.ch1` at the session rate with peaks →
  place `jam.ch0` on a track → **bounce, and it is audible**. Also asserts the take report (channels,
  sources, frames).
- `recording_refuses_what_it_cannot_do` — no pool ("the refusal names the fix"), one take at a time,
  stopping nothing, an invalid take id (the media layer's filename rule), and the grammar
  (`record jam1` / `record stop`).
- `spikes/tui-shell`: `the_command_line_records_and_reports_refusals` — `: record stop` with nothing
  recording reports the host's refusal, a stray word is a parse error, and the prompt still works after.
- 28 host unit tests, 28 TUI tests, 24 workspace test binaries, clippy and fmt clean.

## Review

The stand-in gate (**GLM-5.3**; the designated Kimi gate is down) returned **`do not merge`** — and it
was right: `media::devices::fill_input` pushed **channel 0 only** into a mono ring (by design, "the
mirror of `fill_output`"), while `record` sized the capture from the device's channel count, so
**every multi-channel take was silently corrupt** (channel 1 was channel 0's odd samples, half the
duration, and the report's channels/sources were lies). The hermetic test could not see it: the ring
*seam* feeds genuinely interleaved data, and the device path had no coverage.

Fixed in the same slice: `fill_input` pushes every channel interleaved; `InputHandle` carries the
**stream's** channel count and `record` sizes the capture from that one handle; a unit test pins the
interleaved contract and a hardware-gated `#[ignore]`d test asserts a real device yields its own
channel count at **full duration**; `stop_recording` reports the take even when the stop complains;
re-recording over an existing take id is refused; and a rebuild (seek/Load/undo) **stops** a take in
progress instead of dropping it, with the report surviving the rebuild. The review's arity gaps were
already closed by the `exact` sweep in the same round.

Debt it leaves open (recorded, not fixed): a mid-take `Pool::list` indexes a placeholder-header source
until the take stops; `record`/`record stop` run on the actor thread, so the device open, the demux join
and the peaks write stall the audio pump (an in-playback stop can glitch) — the fix is to open the device
off-thread and hand the stream back; takes are one-at-a-time with no take/comp model. The review is
archived verbatim in
[`research/architecture/2026-09-23-alpha-slice-gate-a3-glm-standin.md`](../../../../research/architecture/2026-09-23-alpha-slice-gate-a3-glm-standin.md).

## Alternatives considered

- **Record in `media` with its own CLI/tool, never in the host.** Rejected: the take must land where the
  session's material lives (the pool, at the session rate) and be visible to the shells; a separate tool
  would duplicate the pool's rules and the drift compensation's parameters.
- **Record as *state* in the log** (so a replay re-runs the capture). Rejected: a performance is not
  reproducible from a command list, and a replay would have to either re-record (impossible) or silently
  drop the take. The take is ground truth; the log records the clips made from it.
- **A background recorder thread with a command channel** (the recorder owns the device, the host sends
  it messages). Rejected for now: the actor thread already owns the output device and the session, so a
  second owner adds a protocol and a lifetime without removing the `!Send` constraint. Revisit if
  recording ever needs to survive a session rebuild.
- **Align every take to the playhead at record time** (record from the transport's frame). Rejected as
  premature: it needs the fixed MIDI→hardware→USB offset per device, and a wrong offset is worse than an
  honest "this take starts at frame 0 of the take". The take is raw material; alignment is an edit.
- **Monitoring the input through the graph as you record** (mount `CaptureNode`s into the mixer so you
  hear the input). Deferred to the pool-panel/audition slice: it is a *graph* change (a second source
  path into the mixer) and the monitoring rings already exist in `Capture` for whoever mounts them.
- **A count-in / metronome before the take.** Deferred to the grid slice (C): a click needs the grid, and
  recording without one is what the owner's hardware does anyway.
- **Write to a temp file and import on stop.** Rejected: the pool's crash recovery already handles a take
  that dies mid-write (the WAV header is written before any audio), so a second copy would only double the
  disk traffic.
- **Silently ignoring extra words** (the parser's old behaviour). Rejected by the review's arity-debt
  item once a test proved the consequence (a stray word became a take id and opened the device).

## Consequences

- The loop's first half works: record → the take is in the pool → place it on a track → arrange → render.
  The shells reach it through the `:` prompt (`: record jam`, `: record stop`); a key or a record button
  is a workflow decision for the pool-panel slice.
- A re-record with the same take id **overwrites** the pool files (the WAV is created fresh); that is
  honest for a retake but is not yet a take/comp model — takes and comping stay out of alpha.
- The offline reference host still cannot record (a script run has no device): `run_script` with a
  `record` line fails cleanly at the device open, which is the right behaviour for a file-driven render.
- A take in progress is **dropped** by a session rebuild (`replay_to`, `Load`): the session is replaced,
  so the stream and the capture go with it. The take's partial WAV remains in the pool and `Pool::recover`
  finalizes it, but the shell is not yet told that it happened.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-23.
