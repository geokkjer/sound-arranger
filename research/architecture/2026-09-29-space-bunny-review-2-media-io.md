# Space-bunny external review — media crate — storage, capture, realtime buffers

> **Disposition.** External deep review, 2026-09-29, against `49e8774` (clean tree). Reviewer:
> Space Bunny (`opencode/space-bunny-free`) driven through OpenCode by DeepSeek Harness; the
> supervisor (DeepSeek-V4.1-Flash) wrote the scope, then handed every CRITICAL/MAJOR claim to an
> **independent adversarial verifier** that had to falsify it before it counted.
>
> This file is the **media crate — storage, capture, realtime buffers** scope, verbatim: the reviewer's report, then its verifier's
> verdicts. What was done about each finding — fixed, deferred or refuted — is in
> [the index](2026-09-29-space-bunny-review.md). Findings are the reviewer's, not the
> supervisor's: read the verdicts before acting on any single claim.

---

## 1. Reviewer's report (verbatim)

## media crate (storage, capture, realtime buffers) review

### MAJOR

**1. `WavWriter::recover` infers bytes-per-sample from the bit depth, so a 24-bit PCM source is silently truncated to two thirds and the returned frame count lies**

- **Severity**: MAJOR (silent data destruction on a user-supplied file; becomes CRITICAL the moment `Pool::recover` is wired into session open, which `pool.rs`'s own module doc already anticipates)
- **Location**: `crates/media/src/wav.rs:409`, with `crates/media/src/wav.rs:422`
- **Evidence**:
  ```rust
  409:        let data_bytes = data_bytes(frames, h.channels, h.bits == 32)?;
  ...
  422:        f.set_len(h.data_offset + data_bytes)
  423:            .map_err(|e| e.to_string())?;
  ```
  and the arithmetic it calls into, `wav.rs:505-511`:
  ```rust
  fn data_bytes(frames: u64, channels: u16, float: bool) -> Result<u64, String> {
      let bytes_per_sample: u64 = if float { 4 } else { 2 };
  ```
  `float` is inferred as `h.bits == 32`, but `parse_header` (`wav.rs:119-126`) accepts **24-bit PCM** explicitly. For a 24-bit file `h.bits == 32` is false, so `bytes_per_sample` becomes 2 while the real frame is 3 bytes.
- **Trigger**: a 24-bit mono WAV with N frames in the pool directory. `frames = actual_bytes / block_align = 3N/3 = N`; `data_bytes = N * 1 * 2 = 2N`; `set_len(data_offset + 2N)` keeps two thirds of the audio. Stereo 24-bit behaves identically (`6N/6 = N` frames, `N*2*2 = 4N` kept of `6N`).
- **Wrong behaviour**: the file is truncated on disk, and `recover` returns `Ok(frames)` = N — the *pre-truncation* count — so `Pool::recover`'s `report.finalized.push((src.id.clone(), frames))` (`pool.rs:254`) records N recovered frames for a file that now holds 0.67·N. `is_finalized` then returns `true`, so the loss is never revisited. The reader is a second party to this: `Pool::import` copies a rate-matching mono file byte-for-byte (`pool.rs:480 fs::copy(src, &tmp)`), so 24-bit sources legitimately live in a pool.
- **Fix**: derive the width from the parsed format tag, not the depth — `parse_header` must return `format` (or the byte width) in `Header`, and `recover` should use `h.format == FMT_FLOAT` with a `bits / 8` fallback, i.e. `Header::block_align()`'s own `self.bits / 8` term instead of a boolean. Reject anything `recover` cannot describe exactly rather than guessing.
- **Verification**: the only two `Pool::recover` tests (`crates/media/tests/pool.rs:65` and `:90`) use canonical 16-bit files written by `WavWriter`; `wav.rs`'s `crash_recovery_recovers_the_take` and `float_take_crash_recovers` are 16-bit and 32-bit-float; the 24-bit test (`a_twenty_four_bit_wav_reads_back`, `wav.rs:821`) never calls `recover`. Not covered. Secondary manifestation, same function: `is_finalized` defines well-formed as `data_offset + data_bytes == file_len` (`wav.rs:435`), so any third-party WAV with a trailing chunk (`LIST`/`INFO`/`fact`/`cue` — routine in 24-bit files) reads as an unfinalized take and gets rewritten and truncated by the same `set_len`.

**2. A crossfade of 0 or 1 samples discards the incoming clip's first sample — and a one-frame clip becomes entirely inaudible**

- **Severity**: MAJOR
- **Location**: `crates/media/src/stream.rs:398` and `crates/media/src/stream.rs:427-431`
- **Evidence**:
  ```rust
  398:            let crossfade = cmd.crossfade.max(1);
  ...
  427:            let pos = fade.total - fade.remaining;
  428:            let denom = (fade.total - 1).max(1) as f32;
  429:            let t = pos as f32 / denom;
  430:            let g_cur = (std::f32::consts::FRAC_PI_2 * t).cos();
  431:            let g_in = (std::f32::consts::FRAC_PI_2 * t).sin();
  ```
  For `crossfade == 0` (mapped to 1 by `.max(1)`): `total = remaining = 1`, `pos = 0`, `denom = max(0,1) = 1`, `t = 0.0`, so `g_in = sin(0) = 0.0`. The incoming's first sample is popped at `stream.rs:436` and multiplied by zero — discarded — and the output is the *outgoing* sample. The comment two lines above (`stream.rs:425-426`, "t runs 0 → 1 across the window, so the last fade sample is pure incoming") is false at `total == 1`: the only fade sample is pure outgoing. For `total ≥ 2` the last sample has `t = 1.0` and nothing is lost.
- **Trigger**: the `host v1` text form, which is the declared wire schema. `crates/host/src/lib.rs:3402-3404` parses the operand with no minimum, and nothing downstream validates it either (`host/src/media_ops.rs:224` decodes it verbatim):
  ```rust
  3402:                let crossfade = word(&words, 3, at)?
  3403:                    .parse()
  3404:                    .map_err(|_| format!("line {at}: bad crossfade"))?;
  ```
  So `splice 48000 take.wav 0` — the most natural spelling of "splice with no crossfade" — reaches `max(1)`. `host v1`'s own round-trip form (`host/src/lib.rs:2957`) writes whatever value it holds, so a 0 survives a save/load cycle.
- **Wrong behaviour**: a hard cut is emitted one sample late, the new clip's first sample is dropped (audible on a transient or downbeat), and for an incoming clip of **one frame** the clip is completely silent: its single sample is consumed by the fade and multiplied by zero, the ring is then empty with `eof` set, and `pop_sample` (`stream.rs:279`) returns legitimate silence for the rest of the clip's life.
- **Fix**: treat a non-positive crossfade as a true cut — apply the command with no fade at all, or clamp `total` to at least 2 and document that a 1-sample crossfade is a 2-sample window. The cleanest form is to branch on `cmd.crossfade == 0` and set `self.cur = Some(Player::new(cmd.incoming))` directly, which also makes the intent explicit.
- **Verification**: the only splice assertions use `crossfade = 512` (`spike_b.rs:147, 220`; `host/tests/reference_host.rs:102, 238, 345`), where `t` reaches 1.0 and the arithmetic is correct. `crossfade ∈ {0, 1}` and one-frame clips are untested. Note the same `max(1)` appears in `host/src/lib.rs:1505` and `:1529` without validation.

**3. `Capture::start` never validates `input_rate`; a zero rate overflows the demux thread in debug and leaks memory without bound in release**

- **Severity**: MAJOR
- **Location**: `crates/media/src/capture.rs:154-155` (with `crates/media/src/drift.rs:28` and `:64`)
- **Evidence**:
  ```rust
  154:                    let ratio = input_rate as f64 / sample_rate as f64;
  155:                    let session_cap = (n_frames as f64 / ratio).ceil() as usize + 2;
  ```
  and the compensator it feeds, `drift.rs:28` / `drift.rs:64`:
  ```rust
  28:            ratio: in_rate as f64 / out_rate as f64,
  ...
  64:            self.pos += self.ratio;
  ```
- **Trigger**: `Capture::start(..., sample_rate, /* input_rate */ 0, source)`. `ratio` becomes `0.0`; `(512.0 / 0.0).ceil()` is `inf`; `inf as usize` saturates to `usize::MAX`; `usize::MAX + 2` is an overflow — a panic on the demux thread in any debug build (`cargo test`, every developer run), which `Capture::stop` then reports as the misleading `"capture demux thread panicked"` (`capture.rs:236-238`). In release the add wraps to `1`, so `session_cap == 1` and `DriftCompensator::pull_output` writes exactly one sample per batch with `pos` frozen at 0 (`pos += 0.0`), while `drift.rs:70`'s `consumed = (self.pos.floor() as usize).min(self.pending.len())` is 0 and `pending` is never drained.
- **Wrong behaviour**: debug — the take aborts with a message that names the wrong cause. Release — every batch appends 512 samples to `pending` and never frees them (unbounded growth for the length of the take) while the recording is a constant DC value, and no counter moves.
- **Fix**: validate both rates in `Capture::start` next to the existing channel-count check (`capture.rs:65-78`): refuse `sample_rate == 0` and `input_rate == 0` with a message naming the parameter. The asymmetry is the tell — `start_recording` already guards the *other* rate and not this one (`crates/host/src/lib.rs:875-877` checks `self.engine.clock.sample_rate == 0`, then line 893 passes `input_rate` through unchecked). A `ratio.is_finite() && ratio > 0.0` assertion in `DriftCompensator::new` would close it at the source too.
- **Verification**: every in-tree call site passes a real rate — `hardware_input.rs:29`, `p1_2.rs:88/258/295/353` all use `rate, rate`, and `host::record` uses `handle.sample_rate` (`host/src/lib.rs:929-931`). No test passes a zero rate. `DriftCompensator`'s own tests use 48 001/47 999 only.

**4. The allocation bound in `PeakFile::read` is derived from the sidecar's own untrusted `frames` field, so it bounds nothing**

- **Severity**: MAJOR
- **Location**: `crates/media/src/peaks.rs:225-236`
- **Evidence**:
  ```rust
  221:        // Bound the per-level allocation: a valid sidecar's largest level (base) has
  222:        // ~frames/base_bin bins, and higher levels keep halving. A corrupt/hostile
  223:        // sidecar claiming billions of bins must fail loud, not allocate gigabytes
  225:        let max_bins = (frames as usize).div_ceil(PEAK_BASE_BIN).max(1);
  226:        let levels = (level_byte[0] as usize).min(PEAK_LEVELS);
  ...
  230:            let n = u32::from_le_bytes(u32b) as usize;
  231:            if n > max_bins {
  236:            let mut mn = vec![0.0f32; n];
  237:            let mut mx = vec![0.0f32; n];
  ```
  The comment states the intent precisely; the check is self-referential. `frames` at line 218 comes from the file's own bytes 13..21, and `n` from the file's own per-level length field.
- **Trigger**: a 25-byte `.peaks` file in the pool directory — 21 bytes of header with `frames = u64::MAX`, followed by 4 bytes of `n = u32::MAX`. `max_bins = 7.2e16`, so `n > max_bins` is false and the guard passes.
- **Wrong behaviour**: `vec![0.0f32; 4 294 967 295]` is requested twice — 17.2 GB each, 34 GB total — from a 25-byte file, before the first `read_exact` has touched a single page. Under overcommit the first write faults the pages in until the read hits EOF, so the process is OOM-killed or stalls on a corrupt sidecar. The comment's promise ("must fail loud, not allocate gigabytes") is exactly what the code does not deliver.
- **Fix**: bound by the bytes actually available, which is the only unspoofable quantity — take `f.metadata()?.len()` and require `n ≤ (file_len - bytes_consumed_so_far) / 8` before allocating, or `read_to_end` (bounded by `file_len`) and slice the level arrays out of it. `max_bins` can stay as a secondary sanity check, but it must not be the primary one.
- **Verification**: `sidecar_roundtrips` (`peaks.rs:353`) and `p1_2.rs:275` read only well-formed sidecars this crate wrote. No test feeds a corrupt or truncated sidecar, despite `Pool::recover` calling `PeakFile::read` precisely to *detect* corrupt sidecars (`pool.rs:264`).

**5. `pending` is documented as a bounded command buffer but `drain_mailbox` moves the entire unbounded mailbox into it, reallocating on the render path**

- **Severity**: MAJOR (invariant 1: "the render path never allocates")
- **Location**: `crates/media/src/stream.rs:318-322` and `crates/media/src/stream.rs:355-361`
- **Evidence**:
  ```rust
  318:            // Bounded command buffer: pushes beyond the preallocated capacity
  319:            // would allocate on the render path (a documented Phase-0 edge —
  320:            // the profile schedules splices on clean boundaries, so bursts
  321:            // beyond 8 are not expected).
  322:            pending: VecDeque::with_capacity(8),
  ```
  ```rust
  355:    fn drain_mailbox(&mut self) {
  356:        if let Ok(mut mbox) = self.mailbox.try_lock() {
  357:            while let Some(cmd) = mbox.pop_front() {
  358:                self.pending.push_back(cmd);
  ```
  The comment asserts a bound the code never applies — there is no cap on the loop and no cap on the mailbox (`stream.rs:249 pub fn mailbox()` is a bare `VecDeque::new()`), so the ninth drained command reallocates on the render thread. This is the residue of a prior Minor ("`pending: VecDeque::with_capacity(8)` can allocate on the render path past 8 queued splices (mailbox is unbounded). Bound the mailbox or pre-reserve to the same bound") — neither half was done, and the comment now reads as if it were.
- **Trigger**: a `host v1` script with `play` followed by nine or more `splice` lines. Each is accepted (`host/src/lib.rs:1502-1537` creates a `FilePlayer` and pushes to the unbounded mailbox with no limit), and the first render block's `drain_mailbox` pushes all of them into `pending`. As a bonus, each `SpliceCmd` holds a live reader thread and a 256 KiB ring, so the same script is a thread and memory amplifier even before the realloc.
- **Wrong behaviour**: one `VecDeque` grow (alloc + memcpy + free) on the audio thread per burst beyond eight — a heap allocation on the render path, which `lib.rs:17-20` states as an absolute invariant. With a large script this is hundreds of pending players and hundreds of MB of rings pinned until they are applied.
- **Fix**: bound the drain (`for _ in 0..N { match mbox.pop_front() { … } }`) and cap the mailbox push on the control side with an explicit refusal, so the invariant holds by construction rather than by the comment's claim about what the profile schedules. `VecDeque::with_capacity` is a starting reservation, not a bound.
- **Verification**: `render_path_does_not_allocate_with_media_nodes` (`spike_b.rs:500`) issues exactly one splice, scheduled at frame 0 with `crossfade: 512` — exactly one block — so the retire happens in the *priming* render at line 513, before `MEASURING` is armed at line 515. The measurement window never sees a splice at all. `p1_2.rs:350`'s allocator test has no mailbox traffic.

### MINOR

**6. RIFF word alignment is still ignored when walking chunks (unfixed prior Minor)**

- **Severity**: MINOR
- **Location**: `crates/media/src/wav.rs:134-138`
- **Evidence**:
  ```rust
  134:        } else {
  135:            reader
  136:                .seek(SeekFrom::Current(size as i64))
  137:                .map_err(|e| e.to_string())?;
  138:        }
  ```
  A chunk body of odd length is followed by one pad byte, so the next tag is read one byte early.
- **Trigger**: an imported WAV with an odd-sized unknown chunk before `data` (a `cue ` point table, an `id3 ` chunk, or any odd `LIST` payload) — common in files from other tools, which `Pool::import` accepts and copies verbatim (`pool.rs:480`).
- **Wrong behaviour**: the next 8-byte chunk header is misaligned, so `data` is not found and `parse_header` fails with `missing data chunk` — a valid file is refused. The same omission exists in the `fmt`-overshoot seek at `wav.rs:88-92` (an odd `fmt` size over 40 bytes leaves the reader one byte off).
- **Fix**: `let pad = size & 1;` and seek `size as i64 + pad as i64` in both the `fmt` and the unknown-chunk arm.
- **Verification**: this was listed in the 2026-08-24 review's Minors and is unchanged; no test covers a non-canonical chunk layout.

**7. `finalize` leaves the writer positioned at byte 44, so a `write` after `finalize` overwrites the audio from the start of the data chunk**

- **Severity**: MINOR (latent public-API hazard; no in-tree caller does it)
- **Location**: `crates/media/src/wav.rs:386-394`
- **Evidence**:
  ```rust
  386:    pub fn finalize(&mut self) -> Result<(), String> {
  387:        if self.finalized {
  388:            return Ok(());
  389:        }
  390:        self.writer.flush().map_err(|e| format!("flush: {e}"))?;
  391:        patch_sizes(&mut self.writer, self.frames, self.channels, self.float)?;
  392:        self.finalized = true;
  ```
  `patch_sizes` seeks to 4, writes, seeks to 40, writes, and returns with the `BufWriter`'s cursor at 44. It never seeks back to the end, and never flushes the eight patch bytes — they sit in the buffer until the `BufWriter` drops.
- **Trigger**: any `write` after `finalize` on a live writer (the guard at 387 only makes `finalize` idempotent, not `write`).
- **Wrong behaviour**: the next `write` starts at byte 44 and overwrites the take's audio from the beginning while `self.frames` keeps counting, so the declared size grows and the file becomes a spliced mix of the two writes. The unflushed patch also means `is_finalized` reports `false` for a finalized-but-not-yet-dropped writer — harmless today because every caller finalizes immediately before dropping, but it is the same missing flush.
- **Fix**: seek back to the end (`SeekFrom::End(0)`) and `flush()` inside `finalize` after the patch, and make `write` return an error once `finalized` is set.
- **Verification**: every in-tree caller writes then finalizes then drops (`lib.rs:83-84`, `pool.rs:300-302`, `capture.rs:279`, `record.rs:87-107`). No test writes after finalizing.

**8. `Capture::frames()` reports the last channel's pull count whether or not that channel's write succeeded**

- **Severity**: MINOR
- **Location**: `crates/media/src/capture.rs:165` and `crates/media/src/capture.rs:182`
- **Evidence**:
  ```rust
  164:                        let n_ok = comps[k].pull_output(&mut session);
  165:                        last_ok = n_ok;
  166:                        session.truncate(n_ok);
  167:                        if let Some(pc) = &mut writers[k]
  168:                            && let Err(e) = pc.push(&session)
  169:                        {
  170:                            *err2.lock().unwrap() = Some(e);
  171:                            writers[k] = None; // stop hammering a failed writer
  172:                        }
  ```
  ```rust
  182:                    frames2.fetch_add(last_ok as u64, Ordering::Relaxed);
  ```
  `last_ok` is assigned inside the per-channel loop, so only the *last* channel's count survives, and it is added regardless of whether `writers[k]` accepted the samples.
- **Trigger**: a write failure on channel 0 (disk full, I/O error) while channels 1..3 keep succeeding — e.g. a pool directory on a full volume.
- **Wrong behaviour**: `frames()` reports N (channel 3's count) while `take.ch0.wav` holds fewer frames. `host::stop_recording` puts that number straight into `TakeReport.frames` (`host/src/lib.rs:959`), which the shell uses to place clips — so clips are created at a length channel 0 cannot deliver. This is the residue of the prior Minor "a failed channel writer keeps the take 'running'; `frames()` over-reports"; the writer is now retired, but the accounting still is not distinguished per channel.
- **Fix**: accumulate per-channel counts and report the minimum written (or a per-channel vector), so a short stem is visible rather than averaged away.
- **Verification**: the capture tests all succeed on every channel, so `last_ok` is identical across `k` and the aliasing is invisible.

**9. Device errors after `play()` are stored in a mutex that no handle exposes (unfixed prior Minor)**

- **Severity**: MINOR
- **Location**: `crates/media/src/devices.rs:119` + `141-148` (output), `crates/media/src/devices.rs:308` + `330-335` (input)
- **Evidence**:
  ```rust
  119:    let err: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
  120:    let err_cb = {
  121:        let err = err.clone();
  122:        move |e: cpal::Error| *err.lock().unwrap() = Some(format!("output stream: {e}"))
  123:    };
  ```
  ```rust
  141:    Ok(OutputHandle {
  142:        stream,
  143:        sample_rate,
  ...
  148:    })
  ```
  The callback's `Arc` clone lives inside cpal and keeps writing, but the local `err` is dropped when `open_output` returns and neither `OutputHandle` nor `InputHandle` carries the handle to read it. The only check is the one-shot `take()` at line 138, which can only catch a failure during `play()`.
- **Trigger**: any runtime device error after startup — the ALSA stream being invalidated, a failed XRUN recovery, a USB disconnect.
- **Wrong behaviour**: the error is written into a mutex nobody reads. The stream goes silent, `underruns` climbs (which reads as a *starvation* diagnosis, not a device failure), and the user is never told the device died. This is the prior review's Minor verbatim, unchanged.
- **Fix**: put the `Arc<Mutex<Option<String>>>` on both handles as `pub error: Arc<Mutex<Option<String>>>`, and surface it in the host's status/`underruns` reporting so a device failure is distinguishable from a ring starve.
- **Verification**: no test covers a post-open device error; `devices_open_and_run` (`spike_b.rs:603`) only asserts the ring drained and prints the input count.

**10. The >4 GiB guard fires only at finalize, after hours of writing, and `Drop` discards the error that would have said why**

- **Severity**: MINOR
- **Location**: `crates/media/src/wav.rs:443-446` and `crates/media/src/wav.rs:350-369`
- **Evidence**:
  ```rust
  443:    fn drop(&mut self) {
  444:        if !self.finalized {
  445:            let _ = self.finalize();
  446:        }
  ```
  `write` (line 350) has no size check, so `self.frames` grows unbounded; the refusal lives in `data_bytes` (`wav.rs:515`) and is only consulted by `finalize`/`patch_sizes`.
- **Trigger**: a take crossing 4 GiB — ≈6.2 h mono float at 48 kHz, which the crate's own doc calls out as the reason the guard exists ("the pool records long live jams"). The take writes to completion, then `finalize` returns `Err`.
- **Wrong behaviour**: the user records for six hours and learns the reason only at the end, and if the writer is dropped rather than finalized the reason is discarded entirely. The audio itself survives (the reader's file-length fallback at `wav.rs:172-175` still reads it), so this is a diagnosability defect rather than data loss — the prior critical #3's core fix holds.
- **Fix**: check the projected size in `write` (or every N frames) and fail with the same message while the take is still short, and log the dropped `Drop` error somewhere observable rather than `_ =`.
- **Verification**: `data_bytes_guard_refuses_oversized_take` (`wav.rs:774`) tests the arithmetic function directly, not the writer's behaviour over a long take, and `float_take_crash_recovers` is 777 frames.

### NIT

**11. `PeakBuilder::push` still panics on a public path** — `crates/media/src/peaks.rs:49`, `assert!(!self.finalized, "peaks: push after finalize");`. The prior review's finding on this function was *partly* fixed — `base_minmax` and `range_minmax` are now bounds-safe and return `Option` (the `peaks.rs:79-91` and `:131-147` rewrites are correct and well-commented) — but the `push` assert remains on a type re-exported from `lib.rs:59`. `PeakFile::write` finalizes the builder (`peaks.rs:169`), so any caller that reuses a builder across writes panics. No in-tree caller does (`pool.rs:336` makes a fresh builder per rebuild). Make it a no-op or an `Err`-returning method to match the rest of the module.

**12. The demux thread overwrites the error state, losing the root cause** — `crates/media/src/capture.rs:170` and `crates/media/src/capture.rs:189` both do `*err2.lock().unwrap() = Some(e);`. A failure on channel 0 followed by a failure on the peaks write for channel 3 leaves only the second message, so the user is told about the sidecar and never about the disk. `if err2.lock().unwrap().is_none() { *err2.lock().unwrap() = Some(e) }`, or `get_or_insert`, keeps the first.

**13. `let _ = handle.join()` reports success on a dead writer** — `crates/media/src/record.rs:161-165`: the join result is discarded, so if the writer thread panics (an allocation failure inside `writer.write`'s `Vec::with_capacity` at `wav.rs:353`, or a poisoned `err` mutex) `stop()` falls through to `self.err.lock().unwrap().clone()` — still `None` — and returns `Ok(())` for a take that was never finalized. The file survives via `WavWriter`'s `Drop` during unwinding, but the caller is told the stop succeeded. At minimum, surface a `Err("recorder thread panicked")` as `Capture::stop` does at `capture.rs:236-238`.

**14. `fill_input` pushes a trailing partial frame where `fill_output` deliberately refuses to** — `crates/media/src/devices.rs:377-383`:
  ```rust
  377:    for frame in data.chunks(channels) {
  378:        for sample in frame {
  379:            if !ring.try_push(sample.to_float_sample()) {
  ```
  `chunks` yields a short final chunk when `data.len() % channels != 0`, so a partial frame can reach `Capture`'s demux. cpal guarantees whole frames, so this is unreachable today — but the asymmetry is notable: the output side has an explicit guard *and* a regression test for exactly this hazard (`devices.rs:450-466`, `output_partial_frame_starves_without_misaligning`), and the doc comment at `devices.rs:361-364` claims the ring "carries what `Capture` demuxes — `channels` interleaved samples per frame". Mirroring the output side's `len() >= source_channels` check would make that claim hold by construction rather than by backend behaviour.

### Test gaps

- **No test drives `fill_output`/`fill_input` through a real device.** The prior review's critical #1 fix is unit-tested against `fill_output` directly (six cases, `devices.rs:393-480` — genuinely good), but the review also asked for "a hardware test asserting channel duplication" and none was added: `devices_open_and_run` (`spike_b.rs:603-648`) only asserts the ring drained and prints a count. A ramp through a real stereo device, asserting L/R values, is the one test that would catch a cpal-backend surprise the unit tests cannot see.
- **The no-alloc test never measures a player retirement.** `render_path_does_not_allocate_with_media_nodes` schedules its splice at `at_frame: 0` with `crossfade: 512` (`spike_b.rs:510`) — exactly one block — so the fade completes and the old player is retired inside the *priming* render at line 513, before `MEASURING` is armed at line 515. The prior review named this exact gap ("its retirement happens during priming, before measurement") and it is unchanged, so the fix for critical #2 remains unverified by the suite that exists to verify it. A splice at a mid-measurement frame with a short crossfade would close it.
- No test issues nine or more splices (finding 5), uses `crossfade ≤ 1` or a one-frame incoming clip (finding 2), calls `WavWriter::recover` on a 24-bit or non-canonical-layout file (finding 1), feeds a corrupt `.peaks` with an inflated `frames` (finding 4), or passes `input_rate == 0` (finding 3).
- `soak_25_minutes_stream_record_splice_bounce` is `#[ignore]`d, so "no overruns over 25 minutes" and the minute-15 splice are never exercised in CI — the long-jam claim the >4 GiB guard and the soak both defend is untested by default.
- `render_path_does_not_allocate` and `capture_to_mixer_render_path_does_not_allocate` both count *allocations* only; neither counts `dealloc` calls, so a free on the render path — the very thing critical #2 was about — is invisible to both. A `dealloc` counter in the `CountingAllocator` would make the invariant testable in the direction that matters.

### Design risks

- **The retire-free fix rests on a timing argument, not a guarantee.** Parking the reader at EOF (`stream.rs:176-185`) is structurally right: the render thread's drop is four atomic decrements, while the reader's exit path is a thread return, a `BufReader` close, and TLS teardown, so the 256 KiB ring's final free lands on the reader thread. But a player retired while its reader is still mid-read exits at `stream.rs:140` or `:154` *without* parking (both `return` before `eof2.store(true)`), and then both threads race to drop the last `Arc<Spsc>`. The render thread winning is overwhelmingly likely and essentially never observed; it is not impossible. A `dealloc` counter (above) plus a test that retires a *long* clip mid-read would turn this from an argument into a measurement.
- **`is_finalized` defines "well-formed" more strictly than a player would.** `data_offset + data_bytes == file_len` (`wav.rs:435`) rejects any WAV with a trailing chunk, so routine third-party files read as crashed takes. Combined with finding 1, the crash-recovery pass is both over-eager (it fires on files that were never ours) and wrong when it does fire. `Pool::recover` has no production caller yet — only `crates/media/tests/pool.rs:74` and `:97` — so nothing is destroyed today, but `pool.rs:1-4` documents this pass as the session-open behaviour, so wiring it up is what turns finding 1 into an incident.
- **`open_input` does not negotiate the rate and `InputHandle` carries no `rate_mismatch`** (`devices.rs:297-336`), the exact asymmetry `OutputHandle` exists to prevent — its own doc at `devices.rs:35-37` says "a 48 kHz session on a 44.1 kHz device would run ~8% flat", which is the *output* symptom. On the input side the symptom is invisible: the take is resampled into session frames by `DriftCompensator`, a documented 2-tap linear interpolator (`drift.rs:7-8`, `:62`). That is exactly right for the ±0.002 % deviations it was designed for, and exactly wrong for a real rate gap — no band-limiting, where the crate already ships `resample.rs` for precisely this. The user gets a silently resampled take and no warning.
- **`media::bounce` cannot bounce a session whose master is wider than stereo.** `lib.rs:82` passes `out_channels()` to `WavWriter::create`, which refuses anything but 1 or 2 channels (`wav.rs:324-328`) — so a session running the adaptable `mixer channels=4` (the capture profile's own layout, exercised in `p1_2.rs:109`) fails to bounce. The failure is loud and the doc at `lib.rs:75-76` only describes mono and stereo, so this reads as a known limit rather than a bug; it will read as a bug the first time someone bounces after a four-channel capture.
- **`pending` is applied in arrival order, not frame order.** `stream.rs:392`'s `pop_front_if(|cmd| cmd.at_frame < f1)` takes the *first* queued command whose frame is due, and the engine's scheduler orders by frame, so a control side that issues out of frame order gets its splices applied out of order — the later one is counted as `deferred` and lands at the wrong frame. Not reachable from the current host (it issues splices at their scheduled frames), but the `deferred` counter would report it rather than prevent it.
- **`Capture` allocates a fresh `Vec` per channel per batch** (`capture.rs:158` `Vec::with_capacity(n_frames)`, plus `session` at `:163` and `chan` per channel) — 3 allocations per channel per 512-frame batch, on the demux thread. The prior review flagged this as churn only, and it is off the audio path, but at 64 channels it is ~200k allocations per second of take. Hoisting the scratch out of the loop (`buf[f * channels + k]` via `step_by`, as the prior review suggested) is a small change.

### Checked and clean

- **`ring.rs`** (all 212 lines). The count protocol is correct: the producer's own `fetch_add` RMWs sit in `count`'s modification order, so its `load(Acquire)` at line 74 can never read a value *lower* than the truth (conservative — it may falsely report full), and symmetrically the consumer's `load(Acquire)` at line 92 can never read a value *higher* than the truth (so it cannot over-pop); the Release/Acquire pair on `count` supplies both happens-before edges (slot write → producer `fetch_add` → consumer `load`; consumer `take` → `fetch_sub` → producer `load`), and `head`/`tail` are single-writer and never read across threads, so their `Relaxed` stores are correct. Full/empty edges are conservative in the right direction on both sides. The prior review's false-sharing Minor is genuinely fixed: three 64-byte sections with a compile-time `offset_of!` assertion (`:34-47`) *and* a runtime line-number test (`:168-179`). `cross_thread_stress` and the wrap tests exercise the edges. `Spsc::new` is the only panic surface (`capacity.is_power_of_two()`), and every call site passes `1 << 16`/`1 << 18` or `DEFAULT_RING_CAPACITY`.
- **`wav.rs` reader path.** Every stack-buffer bound holds for all combinations the header permits: with `bits ∈ {16,24,32}` and `channels ∈ 1..=256`, `chunk_frames = min(256, 1024 / block_align)` guarantees `want * block_align ≤ MAX_CHUNK_BYTES` and `pick + bytes ≤ block_align ≤ 1024`, so `raw[at..at+4]` at `wav.rs:266-282` is in bounds for every legal file — I checked the widest cases by hand (256ch/float: 1 frame/chunk; 256ch/24-bit: 1 frame; 1ch/16-bit: 256 frames). `WavReader::open`'s `file_len - h.data_offset` at line 174 cannot underflow: `data_offset` is `stream_position()` after a successful 8-byte read, so it is `<= file_len` by construction. `block_align() ≥ 2` for every accepted `(channels, bits)`, so the divisions at lines 178, 227 and 248 are safe. Non-PCM (`:97-101`), 8/64-bit (`:119-123`), 32-bit *PCM* (`:124-126`), and `channels == 0` or `> 256` (`:110-114`) are each refused with a specific message, and the zero-width-frame non-progress hazard has a regression test (`:660`). Unknown and oversized chunks seek forward and terminate on read failure — no spin, no hang. `frames_left as usize` at line 256 is safe (bounded by `chunk_frames` regardless, and `usize == u64` on the supported targets). NaN/inf in float samples decode and propagate as data, which is the right call for a float pool source.
- **`wav.rs` >4 GiB guard** (the prior critical #3) is correct and well-reasoned: `data_bytes` uses `checked_mul` for the arithmetic and refuses `> u32::MAX - 37`, and `-37` (rather than `-36`) correctly covers the odd-data pad case the comment explains. `36u32 + data_bytes` at lines 492 and 416 cannot overflow given that bound. `write` clamping NaN via `clamp` and Rust's saturating float→int cast yields 0, not a panic.
- **`snap.rs`** (all 287 lines). Every division is guarded: `step_beats ≥ 0.25` so `nearest`/`floor`/`ceil` cannot divide by zero (`:121-132`), `beats_per_bar.max(1)` (`:48`), and both `beats_to_frames`/`frames_to_beats` refuse a non-positive, non-finite, or zero-rate tempo before dividing (`:145`, `:153`). `beat.max(0.0)` clamps a negative beat, and `f64::max` returns the non-NaN operand so a NaN beat lands on frame 0 rather than propagating. I specifically tried to break `(frame.saturating_add(half) / step) * step` at `:188` — it provably cannot overflow: `q ≤ floor(u64::MAX / step)` implies `q * step ≤ u64::MAX` whether or not the numerator saturates, so the comment's "a frame near `u64::MAX` … must not wrap to a small frame" holds. `Division::next`/`previous`/`label`/`parse` are exhaustive with no missing arms, and the cycle test covers the wrap.
- **`devices.rs` `fill_output`** (the prior critical #1) is genuinely fixed and the fix is complete across the whole mapping table: the per-frame pop with the `len() >= source_channels` guard (`:272`) makes a partial source frame non-consumable so L/R never swap across a starve, `device_channels.max(1)` keeps `chunks_mut(0)` from panicking, and all four channel directions are right — 1→1 mono, 1→2 duplicate (`frame[0]` for every `k`), 2→1 average (`:281-282`, never a silent channel drop), 2→2 passthrough, N→1 duplicate, N→2 `L,R,0…0` (the conventional front-pair mapping). All six have unit tests. `frame` is a 2-element stack array sized to `MAX_SOURCE_CHANNELS`, so no allocation and no stale-slot read. The host's producer side agrees with the framing: `push_stereo` (`host/src/live.rs:495-509`) pushes whole interleaved L/R pairs with an all-or-nothing capacity check, matching `source_channels = OUTPUT_CHANNELS = 2` (`live.rs:333`). Producer and consumer are consistent, which is the failure mode the original bug was.
- **`stream.rs` reader thread.** The loop invariants hold under tracing: `budget = (cycle_len - produced_in_cycle).min(remaining)` cannot underflow, because `produced_in_cycle < cycle_len` is guaranteed at line 142 (either `looping` reset it to 0 at `:168`, or the non-looping case has `cycle_len == want` and `budget == remaining > 0` by the `while` guard), and `budget == 0` — the one value that would make `read_into(&mut buf[..0])` return 0 and be misread as EOF — is therefore unreachable. `phase = off0 % region_len < region_len` guarantees `region_len - phase ≥ 1` at `:134`. `fade.remaining -= 1` at `:438` cannot go negative because the fade is `take()`n the moment it hits 0, and the `expect("checked")` at `:416` is guarded by the `is_none()` check at `:409`. `budget as usize` at `:143` truncates on 32-bit, but the target is x86_64 and the `min(buf.len())` would cap it anyway.
- **`stream.rs` crossfade arithmetic for `total ≥ 2`**: `pos` runs 0…N-1, `denom = N-1`, so `t` spans exactly 0→1 and the last fade sample is pure incoming (modulo `cos(π/2) ≈ 6e-8`). `pos ≤ total - 1` because `remaining ≥ 1` at the top of the body, so `t ≤ 1.0` always and `denom`'s `.max(1)` is only wrong at `total == 1` (finding 2). `pop_sample`'s EOF test (`:279`) is correct on both terms: the `None` branch means the ring is genuinely empty, so `eof`/popped-≥-expected is legitimate silence rather than a lost sample, and the reader's `produced` is deliberately not consulted (the comment at `:253-258` explains the race and the hand-built-`ClipRef` case correctly).
- **`capture.rs` demux frame accounting.** The partial-frame carry (a real bug the e2e test found, per the comment at `:128-134`) is handled correctly, and the padding-at-stop branch is bounded: `n_frames == 0` implies `all < channels`, so `buf.resize(channels, 0.0)` always extends rather than truncates, and the subsequent `drain(..n_frames * channels)` empties the buffer. `session_cap` at `:155` is correctly sized for both drift directions — the maximum number of outputs is `ceil((n_frames - pos) / ratio) ≤ ceil(n_frames / ratio)`, so the `+ 2` margin is sufficient and the `ratio < 1` case cannot cap the output (the comment says exactly this) — and the per-channel `comps[k]` evolution is deterministic given identical-length input, so `last_ok` is the same for every `k` (only the *write* outcome differs; see finding 8). `Capture::start` correctly validates the channel count and hardens the take id against path traversal (`:71-78`) before using it in filenames.
- **Prior findings confirmed genuinely fixed this pass** (checked because the brief asks for regressions, not just re-reports): `parse_script`'s index-out-of-bounds sweep (prior critical #4) — every operand now goes through the checked `word(&words, N, at)?` accessor and a fixed arity via `exact(&words, want, at, …)` on `play`/`stop`/`seek`/`splice`/`record` (`host/src/lib.rs:3342`, `:3350`, `:3379`, `:3393`, `:3412`), with a comment recording the typo that motivated it; `ArrangerNode::new`'s rate-mismatch refusal and `validate_clip` on hand-built tracks (prior Majors #5 and Minor) at `crates/media/src/arranger.rs:128` and `:141-146`; `peaks.rs`'s bounds-safe `base_minmax`/`range_minmax` (prior Minor) at `peaks.rs:83-91` and `:131-147`, with the "an error sentinel must not be representable as data" reasoning intact; and `Spsc`'s cache-line padding (above).
- **`record.rs`.** `stop()` sets the flag, joins (which establishes the happens-before for reading `err`), and returns the error — the ordering is right, and the writer thread's `if stop2.load(Acquire) && n == 0 { break; }` at `:100` guarantees a full drain before `finalize`, so the file holds every pushed frame. `frames`/`overruns` are `Relaxed` counters with no cross-thread ordering requirement, which is correct. `Drop`'s `stop()` is idempotent through the `finalized.swap(true)` at `:156`. `push` correctly checks the stop flag before touching the ring, so the post-stop push in `recorder_ignores_pushes_after_stop` is a real assertion and not timing luck.

---

## 2. Adversarial verification (verbatim)

I'll start by reading the actual code for each claim.

## Verdicts

### `CONFIRMED 2-media-io#1` — but with a correction to the stated trigger

The code defect is exactly as described. `crates/media/src/wav.rs:409` passes `h.bits == 32` as the `float` flag into `data_bytes` (`wav.rs:505-506`), which selects `bytes_per_sample` of 4 or 2 only. `parse_header` (`wav.rs:119-123`) accepts `bits == 24`, and `Header::block_align` (`wav.rs:47-49`) uses the correct `self.bits / 8`, so `frames` is right and `data_bytes` is 2/3 of the file. `f.set_len(h.data_offset + data_bytes)` (`wav.rs:422`) then truncates, and `Ok(frames)` (`wav.rs:425`) returns the pre-truncation count, which `Pool::recover` records at `pool.rs:254`. After the patch `is_finalized` (`wav.rs:435`) is true, so the loss is permanent. `Pool::import` at `pool.rs:480` (`fs::copy`) does put rate-matching mono 24-bit files into a pool verbatim — that half of the claim checks out.

**Correction to the trigger:** "a 24-bit mono WAV with N frames in the pool directory" is not sufficient. `Pool::recover` calls `WavWriter::recover` only when `!src.finalized` (`pool.rs:252`), and a well-formed 24-bit file *is* finalized — `is_finalized` compares `data_offset + data_bytes` against `file_len`, which a ffmpeg/Audacity-style file satisfies exactly. The trigger I used: **a 24-bit mono WAV with a trailing `LIST`/`INFO` chunk** (or any wrong data-size field), which makes `data_offset + data_bytes < file_len`, so `is_finalized` is false and `recover` runs. The reviewer's own secondary paragraph names this, so the finding stands, but the headline trigger is wrong as written. `Pool::recover` itself has no in-tree caller outside `crates/media/tests/pool.rs` — it is a public API with no production wiring yet, which bounds the severity below the reviewer's "silent data destruction on a user-supplied file."

Not covered by any test: `crates/media/tests/pool.rs:65,90` use `WavWriter::create_float` fixtures; `wav.rs:707,759` are 16-bit and float; `a_twenty_four_bit_wav_reads_back` (`wav.rs:821`) never calls `recover`.

### `CONFIRMED 2-media-io#2`

`stream.rs:398` (`cmd.crossfade.max(1)`) and `stream.rs:427-431` are as quoted. For `crossfade ∈ {0, 1}`: `total = remaining = 1`, `pos = 0`, `denom = 1`, `t = 0.0`, `g_in = sin(0) = 0.0`. The incoming's first sample is popped at `stream.rs:436`, multiplied by zero, and dropped; the emitted sample is the outgoing's. The comment at `stream.rs:425-426` is false at `total == 1`.

Trigger constructed from the declared wire schema: `splice 48000 take.wav 0`. `parse_script` (`host/src/lib.rs:3392-3410`) takes the operand with no minimum, `HostCommand::Splice` (`lib.rs:1502-1537`) pushes `SpliceCmd { crossfade: 0 }` verbatim, and `format_command` (`lib.rs:2957`) round-trips a 0 through save/load. The one-frame-clip case checks out too: after the fade, `popped (1) >= expected (1)` makes `pop_sample` (`stream.rs:279`) return legitimate silence.

Two qualifications on the write-up:
- The statement "for `total ≥ 2` … nothing is lost" is wrong in the sense that matters. `pos = total - remaining` is 0 on the *first* fade sample for **every** crossfade length, so `g_in = 0` and `incoming[0]` is always multiplied by zero. The existing test blesses exactly that: `spike_b.rs:260-265` asserts post-fade output equals `fb[xf..]`, i.e. the incoming resumes at index `xf`, having lost `incoming[0]`. The `total == 1` case is not a distinct off-by-one — it is the degenerate instance of the same equal-power form, and what makes it *wrong* is only that the caller asked for a hard cut (`0`) and got a one-sample-late cut with a sample deleted.
- "the same `max(1)` appears in `host/src/lib.rs:1505` and `:1529`" is a misread. Those lines are `crossfade,` (pattern binding) and `crossfade: *crossfade,` (struct field) — a pass-through with no `max(1)`. The absence of validation there is real; the `max(1)` is only in `stream.rs:398`.

Severity MAJOR is generous: 20.8 µs at 48 kHz. The genuine defects are the undocumented `0`/`1` acceptance on the wire schema and the total silence of a one-frame incoming clip.

### `CONFIRMED 2-media-io#3`

`capture.rs:65-78` validates only `channels` and `take_id`; neither rate is checked. `capture.rs:154-155` computes `ratio = input_rate as f64 / sample_rate as f64` and `session_cap = (n_frames as f64 / ratio).ceil() as usize + 2`. With `input_rate = 0`: `ratio = 0.0`, `n_frames as f64 / 0.0 = +inf`, and `f64::INFINITY as usize` saturates to `usize::MAX` (Rust float→int `as` casts saturate). `usize::MAX + 2` then panics with an overflow in dev/test builds (overflow-checks default on; the workspace `Cargo.toml` sets only `lto = true` under `[profile.release]`) and wraps to `1` in release. The panic is on the demux thread, so `Capture::stop` (`capture.rs:235-239`) surfaces it as `"capture demux thread panicked"`.

Release path verified line by line: `DriftCompensator::new(0, 48_000)` sets `ratio = 0.0` (`drift.rs:28`); `pull_output` writes one sample (`pos` stays 0.0, `drift.rs:64`), then `consumed = (0.0).floor() as usize = 0` (`drift.rs:70`) so the `drain` never runs. Every batch appends 512 samples to `pending` and emits `pending[0]` forever — a constant DC take, with `pending` growing unbounded for the take's length. The `frames` counter does move (`capture.rs:182`, +1 per batch), contrary to "no counter moves"; the DC output and the leak are real.

Trigger: `Capture::start(&dir, "t", 1, 48_000, /*input_rate*/ 0, ring)` — a legal call to a public API. The reviewer correctly discloses that no in-tree caller does this (`hardware_input.rs:29`, `p1_2.rs:88/258/295/353`, `host/src/lib.rs:929-931` all pass a real rate) and that `start_recording` (`host/src/lib.rs:861-909`) guards `sample_rate == 0` (line 875) but not `input_rate` (line 893). This is a missing-guard defect, reachable only by a caller that passes 0 today.

### `PARTLY 2-media-io#4`

Right about the code, wrong about the consequence. The self-referential bound is real: `frames` is read from the file at `peaks.rs:218`, `max_bins` is derived from it at `peaks.rs:225`, and `n` is read from the same file at `peaks.rs:230` — so a 25-byte sidecar (`SPK1` + base_bin + level byte + `frames = u64::MAX` + sample_rate + `n = u32::MAX`) yields `max_bins = 7.2e16` and passes the guard at `peaks.rs:231`, then requests `vec![0.0f32; 4_294_967_295]` twice.

But the stated failure mode is backwards. The read loop is:

```rust
for v in mn.iter_mut() {
    f.read_exact(&mut u32b).map_err(|e| e.to_string())?;   // reads into a 4-byte STACK buffer
    *v = f32::from_bits(u32::from_le_bytes(u32b));        // writes mn only on success
}
```

On a 25-byte file the header plus the `n` field consume all 25 bytes, so the **first** `read_exact` fails with `UnexpectedEof` and `?` returns before a single element of `mn` or `mx` is written. The claim's "the first write faults the pages in until the read hits EOF" cannot happen — there is no write. Additionally `vec![0.0f32; n]` goes through `alloc_zeroed`/`calloc`, so a fresh mapping is never memset.

The real consequence is narrower: a 34 GB virtual reservation. Under ordinary overcommit it succeeds lazily and is freed immediately, returning a clean `Err("failed to fill whole buffer")`; under `overcommit_memory=2` or a cgroup memory cap the allocation fails and Rust's `handle_alloc_error` aborts the process. That is a genuine hardening gap — the bound is self-referential, exactly as the kimi note at `research/architecture/2026-08-24-kimi-review-p1-3-3-pool.md` claims it is not — but "OOM-killed or stalls on a corrupt sidecar" overstates it. Reachability is also narrower than stated: `PeakFile::read` has exactly one production caller, `pool.rs:264`, and `Pool::recover` has no in-tree caller outside tests.

### `PARTLY 2-media-io#5`

The mechanism and the trigger are right; the central accusation about the comment is a misreading.

Verified: `drain_mailbox` (`stream.rs:355-361`) is `while let Some(cmd) = mbox.pop_front() { self.pending.push_back(cmd) }` with no cap, and `mailbox()` (`stream.rs:249-251`) is a bare `VecDeque::new()`. `pending` is `VecDeque::with_capacity(8)` (`stream.rs:322`), so the ninth drained command reallocs on the render thread. `VecDeque::with_capacity` is a reservation, not a bound — that part is correct.

Constructed trigger: a `host v1` script of `mount mixer` / `play a.wav ch0` / nine `splice <frame> <b>.wav 512` lines / `bounce`. `HostCommand::Splice` is not in the `at_frame()` list (`host/src/lib.rs:412-427`), so `process` (`lib.rs:2716`) does **not** render forward between splices; all nine accumulate in the mailbox and the bounce's first block drains all nine at once. Each `SpliceCmd` also carries a live reader thread and a 256 KiB ring (`DEFAULT_RING_CAPACITY = 1<<16` f32), so nine is nine threads.

The reviewer's test-coverage analysis is also correct: `spike_b.rs:500` schedules one splice at frame 0 with `crossfade: 512` — exactly one `BLOCK` (`engine/src/graph.rs:27`) — so the fade completes inside block 0 of the priming `render_into` at line 513, before `MEASURING` is armed at line 515. The measured window contains no splice at all.

Where it goes wrong: "The comment asserts a bound the code never applies" and "the comment now reads as if it were [done]." Read `stream.rs:318-321` in full — "Bounded command buffer: pushes beyond the preallocated capacity **would allocate on the render path (a documented Phase-0 edge** — the profile schedules splices on clean boundaries, so bursts beyond 8 are not expected)". The comment names the over-capacity allocation as a known, accepted edge. The label is a loose noun phrase; the substance is honest, and it matches the original Spike B commit `f3b27b2`. "Hundreds of MB of rings" also needs ~1000 splice lines; nine is 2.25 MB. What remains is a real but already-documented Phase-0 edge whose severity is one small `VecDeque` realloc — not a MAJOR, and not a comment that lies.

### New findings

**1. A *16-bit* pool WAV with a trailing chunk silently gains garbage frames — the opposite failure from the 24-bit one.** `crates/media/src/wav.rs:403-422`

```rust
let actual_bytes = file_len.saturating_sub(h.data_offset);   // includes the trailing chunk
let frames = actual_bytes / h.block_align();                  // 16-bit: (2000 + 26) / 2 = 1013
let data_bytes = data_bytes(frames, h.channels, h.bits == 32)?;  // 16-bit is CORRECT here
...
f.set_len(h.data_offset + data_bytes)                          // 2070 → no truncation
```

Take a 16-bit mono WAV: 44-byte header, 2000 bytes of audio, then a 26-byte `LIST` chunk. `is_finalized` (`wav.rs:435`) sees `44 + 2000 != 2070` → false, so `Pool::recover` (`pool.rs:252`) calls `recover`. `frames` comes out 1013, `data_bytes` is 2026, the header now declares 2026 audio bytes, and `set_len` does not shrink the file. The 13 frames of ASCII `LISTINFOISFT...` are now declared audio. `WavReader::open` reports `total_frames = 1013` and `Pool::list` publishes it. The take is permanently 13 frames of noise longer than it was. The 24-bit bug truncates; this one inflates, and the reviewer's secondary paragraph asserts both are "truncated", which is wrong for 16-bit.

**2. `Pool::recover` mutates well-formed third-party WAVs, contradicting `pool.rs`'s own module doc.** `crates/media/src/pool.rs:6-8` vs `pool.rs:252-259`

```rust
//! Sources are immutable *post-finalize*; `Pool::recover` only finalizes crashed
//! takes (patches the header + truncates a torn tail) and *derives* missing
//! `.peaks` — it never mutates a well-formed source.
...
for src in &index.sources {
    if !src.finalized {                      // <-- a well-formed WAV with a trailing
        match WavWriter::recover(&src.wav) { //     chunk is NOT "finalized" by this
```

A DAW-exported 24-bit or 16-bit WAV carrying `LIST`/`INFO`/`fact`/`cue` after `data` is well-formed RIFF and passes `WavReader::open`, yet `is_finalized` classifies it as a crashed take and `recover` rewrites it. The pool's stated invariant ("never mutates a well-formed source") does not hold for any WAV this crate did not write, and `recover` has no format check to fall back on — `parse_header` accepts 24-bit, and `recover` then mis-describes it. This is the reachability framing that finding #1 above does not cover.

**3. `WavWriter::recover` writes a declared data size that is not frame-aligned for 24-bit files, so a reader's own `total_frames` disagrees with the recovery report by construction.** `crates/media/src/wav.rs:409-425`

For a 24-bit mono take of N frames, `data_bytes = 2N` while `block_align = 3`. `set_len` leaves a file whose declared data size is not a multiple of its own frame width, so `WavReader::open` computes `total_frames = 2N / 3` (`wav.rs:178`, floor) while `recover` returned `N`. The pool therefore records N in `report.finalized` and the index later reports `⌊2N/3⌋` for the same file — the two numbers never agree, and neither is the audio that was there.
