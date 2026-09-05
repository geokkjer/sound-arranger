# Second opinion — kimi-cli review of the Spike B media engine

> **Source:** `kimi-cli` non-interactive review, 2026-08-17 (kimi session `session_88ff547d-8e5e-402e-9246-737de0e1f5f7`).
> **Reviewed:** the [Spike B note](../../.agents/notes/implemented/architecture/2026-08-17-spike-b-media-engine.md) design and `crates/media` — `ring.rs`, `stream.rs`, `record.rs`, `drift.rs`, `wav.rs`, `devices.rs`, `tests/spike_b.rs` — against the core invariants (render path never allocates/blocks; same command sequence ⇒ byte-identical bounce).
> **Disposition:** all findings folded in — (critical 1) `FilePlayer::drop` now *detaches* instead of joining, so a player retired by a splice cannot block the render path; (major 2) at most one splice applies per block — a second command stays pending instead of silently overwriting the first fade; (major 3) a splice with no current source fades from silence instead of panicking the audio thread; (major 4) the `Recorder::stop` contract is documented (quiescent transport, single-caller); (minor 5) underrun detection moved to the consumer side (`popped` vs `expected` + `eof()`) — race-free at clip end and correct for hand-built `ClipRef`s longer than the file; (minor 6) `stop()` documented single-caller; (minor 7) deferred (late) splice applications are counted on a shared counter and asserted zero in the splice test; (minor 8) tests warm deterministically (poll `produced()`/`eof()` instead of fixed sleeps), crash-recovery poll raised to 10 s, and the no-alloc test now includes a splice completing *during* the measured render; (nits) ring `count` padded onto its own cache line, `pending` bounded-buffer comment, ring-capacity comment corrected (1.37 s not 5.5 s), fade reaches pure incoming at its final sample (no gain step), `push_input` allocation flagged for Phase 1's device callback. Full text preserved below.

---

## Critical

**1. Splice completion drops a `FilePlayer` on the render path — blocking `join()` + dealloc in `PlaybackNode::render`**

`crates/media/src/stream.rs` — when the fade finishes, `finished.cur` is dropped inside the sample loop. `FilePlayer::drop` does `stop.store()` then `handle.join()`. In the normal case (splice mid-clip) the old reader thread is still alive, blocked in its 50 µs ring-full sleep or inside `read_into` on disk I/O; `join()` waits for it. The drop also releases the last render-side `Arc` refs, potentially freeing the 256 KiB ring on the audio thread. This violates the spike's own hard invariant ("render must never allocate or block") on the headline feature, and the acceptance test can't see it: `render_path_does_not_allocate_with_media_nodes` renders steady state only — no splice ever completes while `MEASURING` is on, and a `join()` wouldn't show up as an allocation anyway.

Fix: make `FilePlayer::drop` non-blocking — store `stop` and *detach* (drop the `JoinHandle` without joining). The reader thread owns clones of every `Arc` it touches, so nothing it uses is freed early; it exits on its own within ~100 µs and frees the ring on its own thread. Join is only tidiness here, not safety. Alternative: push retired players onto a control-side "reaper" queue drained at command-issue time. Either way, add a splice variant to the no-alloc test so this is covered.

## Major

**2. Two splices due in the same block: the first fade is silently overwritten**

The `while let Some(cmd) = self.pending.front()` loop keeps popping commands as long as they're due, and each iteration overwrites `self.fade = Some(...)`. If two commands have `at_frame < f1`, the first `Fade` (both its `FilePlayer`s) is dropped right there — silently discarding a splice *and* triggering finding 1's blocking join ×2 on the render path. The `if self.fade.is_none()` guard only helps across blocks, not within the loop.

Fix: `break` after starting one fade; the second command stays in `pending` and applies next block (or after the fade completes, per the documented edge).

**3. Splice with no current source panics on the audio thread**

`self.cur.take().expect("a splice needs a current source")`. `PlaybackNode::new(None, mbox)` is public API, and `cur` is also `None` while a previous fade borrowed it (finding 2's window). A panic on the render path kills audio mid-stream — worse than a missed splice.

Fix: treat `None` as "fade from silence" (synthesize zeros for the `cur` leg), or drop the command and count it on a shared "rejected commands" counter so the control side can observe it.

**4. Recorder stop-vs-push race: a frame can be counted but never written**

`push` does check-then-act: load `stop_flag` (false), then `try_push`. `stop()` sets the flag and joins; the writer breaks as soon as it sees `stop && ring empty`. A push that passed its flag check just before `stop()` ran can land in the ring *after* the writer's final drain and exit. The frame is lost from the take, but `frames_pushed` counted it — the counter and the file silently diverge, and there's no overrun to observe. Today's tests are safe only because they stop after the render loop ends.

Fix: at minimum, make the contract explicit — `stop()` is only valid when the render thread is quiescent (transport stopped), enforced with a debug_assert or documented on the `Recorder` trait. The cleaner Phase-1 shape is gating `RecordNode::render` on transport state so no push can be in flight when `stop()` runs. Don't try to close it with a control-side sentinel push — that would make the ring MPSC and break the SPSC contract.

## Minor

**5. `produced < expected` underrun logic has two holes**

- The reader increments `produced` *after* `try_push` succeeds. If it's preempted between the last push and the last `fetch_add`, a fully-streamed clip reads as `produced == expected - 1`, the next empty pop counts a false underrun, and the `assert_eq!(underruns, 0)` tests flake.
- If a hand-built `ClipRef` has `len` beyond actual EOF, the reader breaks early, `produced` never reaches `expected`, and every subsequent pop counts an underrun *forever* — the "legitimate silence" path never engages.

Fix: the race-free source of truth is consumer-side — the node is the only consumer, so track `popped: u64` per player and treat `ring empty && popped == expected` as done, no cross-thread counter read needed. Short of that, consult `eof()` (set with `Release` after the read loop) in the `None` branch.

**6. Concurrent `stop()` returns `Ok` before the file is finalized**

`finalized.swap(true)` makes the second caller return `Ok(())` immediately while the first caller is still inside `handle.join()`. Anyone using a second `stop()` as "the take is on disk, header patched" has a race.

Fix: second caller should wait for completion, or document `stop()` as single-caller.

**7. A missed `try_lock` silently costs sample accuracy**

If the control side holds the mailbox lock when the block drains, a splice due *this* block applies next block; `at_frame.saturating_sub(f0)` clamps to 0 and the fade starts at block start with no counter, no log. The note promises sample-accurate splices; this failure mode is invisible.

Fix: count late/deferred applications on a shared atomic so tests and the control side can see it happened.

**8. Test flake risks (timing-dependent asserts)**

- Fixed warm sleeps + hard `underruns == 0` on a consumer paced at ~10× real time. Fine on a warm page cache, flaky under CI load. Better: wait on a condition (`produced()` reaching a threshold) instead of `sleep(100ms)`.
- The crash-recovery poll caps at 4 s of wall time for the writer to flush 20 s of audio; a stalled disk flakes it. Poll with a more generous bound.

## Nits

- **False sharing** — `head`, `tail`, `count` sit in one struct, almost certainly sharing cache lines; producer and consumer hammer `count` from both sides. Pad them for the real-time path.
- **`pending.push_back` can allocate on the render path** — `VecDeque::with_capacity(8)` grows when more than 8 commands are pending. Bounded in practice, but it's an alloc the no-alloc test never exercises.
- **Wrong comment** — 1<<16 samples at 48 kHz is ≈ 1.37 s, not 5.5 s.
- **Fade never quite reaches pure B inside the window** — `t` maxes at `(total-1)/total`, so the last fade sample has `g_in < 1`, then jumps to pure B next sample. Inaudible at any reasonable `N`; worth a comment since the note says "crossfade of exactly the requested length".
- **`push_input` allocates** — `Vec::extend_from_slice` is fine in the spike's numeric tests, but the compensator's natural home is the input-device callback; flag it before Phase 1 wires it there.

## What kimi checked and found sound

- **Ring protocol** — correct. `count` is the single synchronization variable and the Release/Acquire chains hold in both directions; slot ownership is airtight (producer writes slot `head` only when `count < capacity`, consumer reads slot `tail` only when `count > 0`); SPSC + `T: Send` makes the `unsafe Sync` sound.
- **Fade math across block boundaries** — correct. `offset` pops pure `cur` without decrementing `remaining`, `offset` zeroes at block end; the 60000-frame splice case traces correctly (block 117, offset 96, fade spans into block 118); the post-fade equality check genuinely pins the fade to exactly 512 samples.
- **Drift accumulator** — sound. `pos` stays small, the hold-at-tail consumes every delivered sample, starvation returns short without advancing `pos`, and pending stays bounded for ratios on both sides of 1.
- **Determinism tests are honest** — writer-thread timing can't affect take content (the ring preserves order; an overrun would change byte length and fail the equality assert), and quantization is deterministic.
