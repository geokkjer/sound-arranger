# Reviewer gate (stand-in) — alpha slice A3 (recording), GLM-5.3

> Research input, 2026-09-23. Co-worker: **GLM-5.3** (`zai/glm-5.3`), read-only, standing in for the
> designated gate **Kimi K3** (unavailable: `opencode-go` failing, CLI device login `403`, API window
> exhausted). Scope: slice A3 — `record <take_id>` / `record stop` wired into the host.
>
> **Verdict: `do not merge`, and the blocker was real and mine.** `media::devices::fill_input` pushed
> **channel 0 only** into a mono ring (by design: "the mirror of `fill_output`"), while
> `HostSession::record` sized the capture from the device's channel count. So on any multi-channel
> interface the demux read a mono stream as interleaved frames: **every take was silently corrupt** —
> channel 1 was channel 0's odd samples, the duration was halved, and `TakeReport.channels`/`sources`
> were lies. On the stated 4-channel target (Notepad-12FX) it was quarter-rate four-channel garbage.
> The hermetic test could not catch it: the ring **seam** feeds genuinely interleaved data, and the device
> path had no coverage at all.
>
> **Fixed before merge:**
>
> 1. `fill_input` now pushes **every channel of every frame, interleaved** (the ring carries what
>    `Capture` demuxes), with its doc rewritten to say so and to record why the old behaviour was a trap.
> 2. `InputHandle` carries the **stream's own channel count** (`channels`), and `record` sizes the capture
>    from that handle — one device call, no second config query to race a device swap.
> 3. A hermetic unit test (`input_pushes_every_channel_interleaved`) pins the contract, including the
>    overrun count, and a new **hardware-gated** test (`crates/media/tests/hardware_input.rs`,
>    `#[ignore]`d) asserts a real device yields its own channel count at the **full duration** — the
>    shape the bug broke.
> 4. `stop_recording` builds the `TakeReport` **before** propagating a stop error, so a take that
>    finalized with a complaint (a partial tail frame, a peaks write) is still placeable.
> 5. Re-recording over an existing take id is **refused** (it would silently change the content of every
>    clip already referencing `{take_id}.ch{k}`), naming the file in the way.
> 6. A rebuild (`seek`, `Load`, undo) now **stops a take in progress** instead of dropping it with the old
>    session: it is finalized in the pool *and* its report survives the rebuild into the outcome.
> 7. The arity gaps the review listed (`transport play junk`, `play … junk`, `set_tempo … junk`,
>    `patch … junk`) were already closed earlier in the same round by the `exact` sweep, and
>    `a_stray_operand_is_a_parse_error` now asserts all of them.
>
> **Debt it leaves open (recorded, not fixed):** a mid-take `Pool::list` indexes a placeholder-header
> source (0 frames) until the take stops; `record`/`record stop` run on the actor thread, so the device
> open, the demux join and the peaks write stall the audio pump (an in-playback stop can glitch a
> ~0.34 s output ring) — the fix is to open the device off-thread and hand the stream back; takes are
> still one-at-a-time with no take/comp model.
>
> The review is archived verbatim below.

## The review, verbatim

# Review: A3 — recording wired into the host

**Verified** = read in source; **inferred** = reasoned, not directly observed.

## 1. The take is complete and correct — mostly holds, one reporting gap

Order in `stop_recording` (`crates/host/src/lib.rs:614-637`) is sound: `drop(handle)` stops the cpal stream first, then `capture.stop()` sets the flag, drains, finalizes, joins (`crates/media/src/capture.rs:222-231`). Frames are **not** lost in the window: samples already in the source ring survive the stream drop, and the demux only checks the stop flag when it has no whole frames (`capture.rs:125-135`), so it drains everything first. `frames()`/`dropped()` are read after the join — accurate. A second `record` while one runs is refused *before* any device is touched (`lib.rs:586-588`). A session dropped mid-take is also safe: `Recording`'s fields drop in declaration order (handle, then capture, `lib.rs:1703-1708`) and `Capture::Drop` calls `stop()`, which finalizes the WAVs — the take survives.

Gap: if `capture.stop()` returns `Err` — the "stopped with a partial frame" path (`capture.rs:127-133`) or a peaks-write failure (`capture.rs:175-181`) — `stop_recording` propagates it via `?` **after** the take was finalized. The WAVs exist and are indexed, but `last_take` is never set and the `TakeReport` (frames/dropped/sources) is discarded; the shell is told "failed" and cannot place clips from a take that actually landed.

## 2. Pool coherence — argument order is right; re-record is silent overwrite

The argument order at the call site is **correct**: `Capture::start(&pool, take_id, channels, session_rate, input_rate, source)` (`lib.rs:559-566`) matches the signature `sample_rate` then `input_rate` (`capture.rs:52-58`), verified against the drift tests (`assert_drift_corrected` passes `session_sr, input_sr`). Ids are validated `[A-Za-z0-9_-]+` before the thread spawns (`capture.rs:63-70`), and `Pool::list` sees `{take_id}.ch{k}` with the session rate (asserted in the test).

Two real issues: **re-recording the same take id truncates** — `WavWriter::create_float` → `File::create` (`wav.rs:246, 266`) — so any clip already referencing `jam.ch0` silently changes content; nothing warns or refuses. And mid-take, the WAV header holds placeholder sizes (`wav.rs:244`), so a concurrent `Pool::list` indexes a 0-frame source (inferred from the header-placeholder comment plus per-push flush).

## 3. The live actor — no deadlock, but the pump stalls

The `!Send` stream lives in the session on the actor thread, correctly built inside the spawned closure (`live.rs:188`). No deadlock: the input callback only `try_push`es (`devices.rs:360-373`), the demux is a separate thread, and `stop()`'s join is bounded by the ring drain. However, `record` and `record stop` execute via `session.execute` on the actor thread (`live.rs:302-331`) — the same thread that runs `fill_audio`. Opening a device (potentially hundreds of ms on ALSA) plus join + finalize + peaks I/O all stall the pump; with a ~0.34 s output ring (`live.rs:268`), an in-playback `record stop` will glitch playback (inferred timing, but the structure guarantees the stall). `Load` mid-take: the fresh session replaces the old one (`live.rs:333-353`); the take is finalized by `Drop` but never reported — no error, no `TakeReport`. Acceptable as data preservation, not as UX.

## 4. The device path — the channel mismatch is a silent correctness bug

This is the blocker. `fill_input` pushes **one sample per frame — channel 0 only** into a single mono ring (`devices.rs:355-373`, its own doc comment says so, and `input_pushes_one_sample_per_frame` asserts it). But `HostSession::record` sizes the capture from the *config's* channel count: `channels = device_channels.clamp(1, 8)` (`lib.rs:593`), and the demux interprets the mono stream as interleaved N-channel frames (`capture.rs:147-151`). On a stereo interface: ch1 is ch0's odd samples, the take is half the real duration, `frames` is halved, and `TakeReport.channels`/`sources` are lies. On the stated P1.2 target (4-channel Notepad-12FX) it's quarter-rate four-channel garbage. The rate mismatch between the two config calls is refused (`lib.rs:596-603`) — the channel dimension is not checked at all, because `InputHandle` doesn't even carry a channel count. Interleaving assumption between `devices.rs` (mono ring) and `capture.rs` (expects interleaved) is flatly contradictory. Same-device in/out is only a feedback risk, not a correctness issue.

## 5. Arity strictness — holds, but inconsistently applied

`exact` (`lib.rs:2557-2566`) is applied after the `@frame` pop, so `record jam @0` still parses. I grepped the repo's scripts/tests: no valid existing form breaks (`pool`/`save`/`load`/`bounce` paths were already single-token). Still silently ignoring extra words: `transport play junk`, `play path ch0 junk`, `set_tempo 120 4 junk`, `patch a.b c.d junk` — none call `exact`.

## 6. Most likely bug the tests miss + merge blocker

The mono-ring/multi-channel demux mismatch above: every test feeds genuinely interleaved data through the `start_recording` seam (`lib.rs:4044-4055`); the device path is only exercised `#[ignore]`d. I would refuse to merge without `fill_input` pushing all channels interleaved (and `open_input` reporting the stream's real channel count so `record` sizes the demux from the stream, not a second config query), plus a hardware-gated test asserting a stereo device yields 2 real channels at full duration. Also recommended: report the `TakeReport` even when `capture.stop()` errors, and surface a Load-dropped take.

(Note: read-only review — I did not run `cargo test`, which would write `target/` and temp dirs.)

VERDICT: do not merge: the device path records ch0-only into a mono ring but demuxes it as `device_channels` interleaved — every multi-channel take is silently corrupt; fix `fill_input`/`open_input` channel plumbing and guard with a real-device test before merge.
