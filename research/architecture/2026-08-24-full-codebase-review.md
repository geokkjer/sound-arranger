# Full-codebase review — architecture and implementation

> Research input, 2026-08-24. Scope: all three crates (`engine`, `media`, `host`, ~7,700 lines of Rust) — architecture and implementation, with emphasis on realtime safety, audio correctness, and Rust quality per the rust-best-practices skill. Signals at review time: 131/131 workspace tests pass, `cargo clippy --workspace --all-targets --locked -- -D warnings` clean. All critical findings verified line-by-line by the reviewer, not just reported by subreviewers.

## Verdict

**Architecture: genuinely excellent. Implementation: correct at its tested edges, with 4 verified critical bugs at untested edges.** The core design (clock · graph · log · ctx, everything-else-a-plugin) is disciplined, honest about its gaps, and the two headline invariants — byte-identical replay and allocation-free render — are *tested properties, not claims*. The bugs live exactly where the test suite can't see them: real stereo hardware, retired-player memory frees, >4 GiB takes, and malformed wire input.

## What's right (worth keeping firmly)

- **Layering is real, not aspirational**: `engine` is std-only, cpal quarantined in `devices.rs`, `media` mounts as opaque nodes with zero core changes. The UI-as-plugin seam is *provable* — the headless host exercises the whole contract.
- **The two-phase mutation discipline** (validate sync → log → apply at frame) with "a refused mutation is never logged" is consistently enforced and tested (`refused_commands_are_rejected_identically`).
- **The SPSC ring's memory ordering is correct**: count-based protocol with Release/Acquire on the right edges, exclusive slot ownership, conservative full/empty. No torn reads possible.
- **Sample-accurate everything**: block-splitting around scheduler events, splice crossfades, unmount boundaries — all pinned by exact-content assertions, not timing luck.
- Crash-recovery design (placeholder WAV header before audio, frame-aligned truncation in `recover`, atomic `.peaks` writes via tmp+rename) is sound.
- Test honesty: timing-lucky assertions were deliberately replaced with exact-content assertions; warm-ups deterministic; `#[ignore]`d hardware tests carry run commands.

## Critical (all verified line-by-line)

1. **`devices.rs` ~line 96 — output callback pops per *sample*, not per *frame*.**
   ```rust
   for d in data.iter_mut() {
       *d = T::from_sample(ring.try_pop().unwrap_or(0.0));
   }
   ```
   The doc comment says "duplicates them across channels" — the code doesn't. On a 2-channel output device (headphones, laptops, the Scarlett), `data = [L0,R0,L1,R1,…]` gets ring samples 0,1,2,3…: each channel plays every *other* sample of the mono stream at half rate. Invisible to CI (hardware tests are `#[ignore]`d and only assert the ring drained). Fix: `for frame in data.chunks_mut(ch) { let s = pop(); for d in frame { *d = s } }`, and extend the hardware test to assert channel duplication.

2. **`stream.rs` ~line 371 — retiring a finished clip frees ~256 KiB on the render thread.** `self.cur = Some(finished.incoming)` drops the old `FilePlayer`; if its reader thread already exited at EOF (the ordinary case — clip A finished before the splice arrived), the audio thread performs the final `free()` of the 65,536-slot ring (256 KiB) plus several `Arc` allocations — violating the crate's headline invariant. The no-alloc test misses it because its retirement happens during priming, before measurement. The "detached reader frees its refs on its own thread" comment is only true while the reader is alive. Fix: park the reader at EOF polling `stop` so it always outlives the player, or a control-side retire drain.

3. **`wav.rs` lines 295–297 (+ `recover` ~343) — data sizes >4 GiB silently truncate.** `patch_sizes` computes `(frames * channels * bytes_per_sample) as u32`; `recover` does `data_bytes as u32`. Threshold ≈ 6.2 h mono float @48 kHz (~3.1 h stereo) — squarely in "record long live jams" territory. The take finalizes with a corrupt small header and reads back short *with no error* (`WavReader` only falls back to file length when the declared size *exceeds* the file). Fix: `Err` when `frames * block_align > u32::MAX - 36`; RF64 is the longer-term answer.

4. **`host/lib.rs` `parse_script` (lines ~549, 566–596) — panics on truncated lines.** Direct `words[N]` indexing in 8 of 9 arms (`mount`, `set_param`, `set_tempo`, `unmount`, `play`, `splice`, `record`, `bounce`): `printf 'host v1\nmount\n' | host` → index-out-of-bounds panic (exit 101), bypassing the designed `bad script`/exit-2 path. The `patch` arm already does it right via `port_ref`'s `words.get(idx)`. This is the declared wire schema for the Tauri shell — a typo'd or truncated UI command would panic the parser. Fix: `words.get(N).ok_or_else(...)` everywhere + parse-error tests per missing operand.

## Major

5. **`drift.rs` is dead code** — zero call sites outside its own tests (verified by grep). No layer reconciles sample rates: a 44.1 kHz take arranged in a 48 kHz session plays pitch-shifted with no diagnostic (`ArrangerNode::new`/`FilePlayer` never compare `WavReader::sample_rate()` to the session rate). Minimal fix: `Err` on mismatch in `ArrangerNode::new`; proper fix: wire `DriftCompensator` per channel (it is mono — one per channel works) on the capture side or at playback.
6. **`host/lib.rs` ~396 — `render()`'s `.expect`s are reachable user paths.** `unmount mixer` after `play` (both valid commands) leaves `pending_cords` populated and clears `graph.out_node` (the mixer disposer removes the node) — any later engine command with `at_frame > now` triggers the pre-render → `wire_pending()` → `Err("play requires the mixer…")` → panic. The comment "wiring is validated at Play apply" is false. Similarly `wire_arranger` panics on an `Arrange` clip whose pool source is missing (resolution happens at wiring time, not apply). The `Bounce` arm does it right (`?` propagation). Fix: make `render()` return `Result<Vec<f32>, String>`, propagate in `run_script`, and clear `pending_cords`/`player_mailbox`/`mixer_channels` on mixer unmount.
7. **The text format cannot express `pool`/`arrange`** (P1.3.4 commands) — no match arms, so the CLI smoke binary cannot exercise the arrangement path, contradicting the crate doc's "the wire schema its commands validate against". Add text forms (the op encoding is precisely the contract decision to make) or scope the claim.
8. **`graph.rs` `MAX_PDC = 64` samples** is far below real lookahead devices (~5 ms limiters ≈ 240 samples @48 kHz); `RingDelay::set_delay` silently clamps (`delay.min(buf.len())`), so a future limiter gets *under-compensated* PDC with no error. Fine for the spike; raise the cap or error before effects land.
9. **Mount params bypass validation entirely** — `mount tone gain=NaN` parses (`"NaN".parse::<f32>()` succeeds) and engine factories silently ignore unknown params (`get(key, default)` pattern); mount params get none of the finiteness/range checks `set_param` has. Fix: per-plugin param lists in the host registry + finiteness at parse, or extend `validate_mount` to check the `params_table` it already holds.

## Minor

- **`ring.rs`** — `head` and `tail` share the first cache line; only `count` is isolated by `_pad`. The producer writes `head` and the consumer writes `tail` on every push/pop, so the hottest structure bounces one line between threads per sample — precisely the false sharing `_pad` set out to prevent. Fix: pad between `head` and `tail` (three 64-byte sections).
- **`stream.rs`** — `pending: VecDeque::with_capacity(8)` can allocate on the render path past 8 queued splices (mailbox is unbounded). Bound the mailbox or pre-reserve to the same bound.
- **`devices.rs`** — stream errors after open are never surfaced (error callback's `Arc` is dropped after `play()`); `InputHandle` hides the channel count (TOCTOU vs `default_input_config`). Put shared error state and `channels` on the handles.
- **Public-API panics**: `peaks.rs` (`assert!`/`panic!` in `push`, `base_minmax`, `range_minmax`), `clip_editor.rs` (`expect("timeline poisoned")` in `snapshot` while `register_handlers` maps poison to `Err` — pick one policy). A UI zoom query hitting a bad bin shouldn't take down the app.
- **`arranger.rs`** — `ArrangerNode::new` accepts a hand-built `Track`, bypassing `Timeline` validation; run `validate_clip` in `new()`.
- **`pool.rs`** — non-UTF8 filename yields `id: ""`, silently unresolvable; report in `PoolIndex::errors`. Related: `wav.rs` chunk skipping ignores RIFF word-alignment (`size & 1`).
- **`capture.rs`** — a failed channel writer keeps the take "running"; `frames()` over-reports. Track per-writer failure distinctly.
- **`host/lib.rs`** — `mixer_channels` is shadow state (`*v as usize` float cast, stale on unmount); `pub fn engine(&mut self)` test scaffolding is a dead, public contract bypass — delete or feature-gate; determinism rests on the undocumented bound `bounce frames ≤ DEFAULT_RING_CAPACITY` (longer bounces can underrun nondeterministically); `bounce 999999999999` → capacity-overflow/OOM abort; parse-error line numbers wrong after leading blanks/comments (`at = lineno + 2` assumes `host v1` is line 1); trailing tokens silently accepted (`play a.wav ch0 junk` parses).
- **Engine nits** — `Graph::control_scratch` as a struct field is a local pretending to be state; `Scheduler::pop` is O(n) `Vec::remove(0)` (control side, documented — fine); `EuclideanGen` calls `beat_at`/`frame_at` per step (O(segments) per block — fine until tempo maps grow); `EventBuf` drops silently at capacity (documented); `frame_at`'s fall-through `.expect` is a true invariant; mixer `set_param` string-parses dotted names on the control side (fine).
- **`Result<T, String>` crate-wide** — defensible under the std-only sibling-of-core constraint; revisit at the Tauri/IPC boundary where errors must be matched, not string-matched (thiserror then).
- **`capture.rs` demux** — per-channel `chan` Vec allocated per batch; hoist scratch and use `buf[k..].step_by(channels)`. Off the audio path, just churn.

## Rust quality assessment

Idiomatic and disciplined throughout: `let-else`, `ok_or_else`/lazy `format!`, Copy types by value, const-generic fixed buffers, `debug_assert` for invariants with release-mode no-op degradation on the render path, narrow `unsafe` with genuine SAFETY cases, destructured borrows for disjoint field access. No `unwrap()` on production paths in `engine`. Tests follow naming and one-behavior-per-test conventions; integration tests are external-only public API; the binary is exercised via `CARGO_BIN_EXE_host`.

## Test coverage — what's pinned vs missing

Pinned well: byte-identical replay (API and through the binary, stdin included), identical logs, splice-vs-control diff, sample-accurate lifecycle, underrun/deferred counters as diagnostics, drift math (both ratio directions, pitch preservation, boundedness), full timeline op semantics including refusals, codec round-trip, crash recovery, pool listing, two independent no-alloc proofs.

Missing (each maps to a finding above): malformed-line parse errors, NaN/inf mount params, huge bounce frames, unmount-mixer-then-render, arrange-with-missing-source, rate-mismatch playback, >4 GiB, recover's torn-tail truncation, corrupt `.peaks` rebuild, deferred-splice counter path (asserted 0 only), actual stereo device output content — which is exactly why critical #1 survived.

## Recommended fix order

1. `devices.rs` stereo fix (+ hardware test asserting channel duplication)
2. `parse_script` `words.get` sweep + malformed-input tests
3. WAV >4 GiB guard
4. Player-retire free (reader parks at EOF)
5. Rate-mismatch refusal in `ArrangerNode::new`; wire `DriftCompensator`
6. `render()` → `Result` + host-state cleanup on unmount

Items 1–4 are small, mechanical diffs; each has a clear test that would have caught it. The project's own standard — invariants as tested properties — is exactly the tool to hold over these edges.
