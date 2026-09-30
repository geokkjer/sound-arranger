# Space-bunny external review — test quality, CI gates and docs-vs-code

> **Disposition.** External deep review, 2026-09-29, against `49e8774` (clean tree). Reviewer:
> Space Bunny (`opencode/space-bunny-free`) driven through OpenCode by DeepSeek Harness; the
> supervisor (DeepSeek-V4.1-Flash) wrote the scope, then handed every CRITICAL/MAJOR claim to an
> **independent adversarial verifier** that had to falsify it before it counted.
>
> This file is the **test quality, CI gates and docs-vs-code** scope, verbatim: the reviewer's report, then its verifier's
> verdicts. What was done about each finding — fixed, deferred or refuted — is in
> [the index](2026-09-29-space-bunny-review.md). Findings are the reviewer's, not the
> supervisor's: read the verdicts before acting on any single claim.

---

## 1. Reviewer's report (verbatim)

## Test quality, CI gates, and documentation-vs-code review

Scope: `crates/{engine,media,host,workflow}` test suites, `.github/workflows/ci.yml` + `.githooks/` + `scripts/`, and `README.md` / `docs/*.md` / crate `//!` docs, at `49e8774`. The prior review's 4 criticals and majors were re-checked: #1 (per-sample output pop), #2 (render-thread free on retire), #3 (>4 GiB WAV), #4 (`parse_script` panics), #6 (`render()` → `Result`), #7 (text form for `pool`/`arrange`), #9 (NaN mount params) are all genuinely fixed and well covered. Findings 3 below are the two prior **Minor**s that were never fixed, plus new material.

### MAJOR

1. **`WavWriter::recover` computes a 24-bit take's data size at 2 bytes/sample, then `set_len`s the file to that size — a third of the audio is destroyed, silently.**
   - **Severity:** MAJOR (silent data loss on a public, documented, tested API)
   - **Location:** `crates/media/src/wav.rs:409`, `crates/media/src/wav.rs:422`, `crates/media/src/wav.rs:505-509`; reached from `crates/media/src/pool.rs:253`
   - **Trigger:** `Pool::recover` calls `WavWriter::recover` for any `.wav` where `is_finalized` is false. `is_finalized` is `data_offset + data_bytes == file_len` (`wav.rs:435`), which is false for (a) any file with a chunk *after* `data` (many DAWs write a trailing `LIST`/`id3 `) or (b) a 24-bit mono file with an **odd** frame count, where RIFF's required pad byte makes `file_len` one larger than the declared size — roughly a coin flip per take. Then:
     ```rust
     let data_bytes = data_bytes(frames, h.channels, h.bits == 32)?;
     ```
     ```rust
     fn data_bytes(frames: u64, channels: u16, float: bool) -> Result<u64, String> {
         let bytes_per_sample: u64 = if float { 4 } else { 2 };
     ```
     `h.bits == 32` is passed as the `float` flag, so 24-bit gets `bytes_per_sample = 2` instead of 3. The header is then patched to 2/3 of the real size and `f.set_len(h.data_offset + data_bytes)` (`wav.rs:422`) **truncates the file**.
   - **Wrong behaviour:** a hand-placed or adopted 24-bit WAV loses its last third of audio, with no error (`recover` returns `Ok(frames)` and the report says the take was "finalized"). A 24-bit file is exactly what `docs/beta-acceptance.md:24` tells a second person to bring, and `Pool::list` indexes 24-bit files as ordinary sources.
   - **Evidence:** `wav.rs:119-126` proves 24-bit PCM is a *supported* reader input, so the writer/recover pair is expected to handle it; only `recover` gets it wrong.
   - **Fix:** pass the real sample width, e.g. change the signature to `data_bytes(frames, channels, bytes_per_sample: u64)` and call `data_bytes(frames, h.channels, (h.bits / 8) as u64)`; or refuse to `recover` a non-16/32-bit file loudly instead of guessing.
   - **Verification:** read `wav.rs` in full, `pool.rs:242-273`, and all four recover call sites in tests. No test recovers a 24-bit file — `crash_recovery_recovers_the_take` (16-bit), `float_take_crash_recovers` (float), `recover_finalizes_a_crashed_take` (float), `crashed_take_is_recovered` (16-bit). Confirmed gap.
   - **Reachability caveat:** the host and both shells never call `Pool::recover` (only `open`/`conform`/`list`/`write_source` — `crates/host/src/lib.rs:741,744,766,1130`). So today this is reachable only from a library caller or the next slice that wires recovery up, which is why this is MAJOR and not CRITICAL.

2. **The media no-allocation test claims to cover the player-retire free during the measured render — the splice fires during the *prime* render, so the coverage claim is false.**
   - **Severity:** MAJOR (a test that does not test what it says; this is the exact hole the prior review named)
   - **Location:** `crates/media/tests/spike_b.rs:507-522`
   - **Trigger:** `schedule_splice(&rig, 0, …)` pushes a `SpliceCmd` with `at_frame = 0`. The prime call `rig.e.render_into(&mut out)` (8192 samples) is chunked at `BLOCK = 512` (`crates/engine/src/graph.rs:27`); in the **first** chunk `drain_mailbox` moves the command into `pending`, `pop_front_if(|cmd| cmd.at_frame < f1)` fires it, and the 512-sample fade retires the old `FilePlayer` on the last sample of that same chunk — all before `ALLOCS.store(0, …)`.
     ```rust
     // a splice that completes *during* the measured render: command issue
     // (thread spawn, warm ring) is control-side; application (fade mix, player
     // retire → detach) must not allocate or block on the render path.
     schedule_splice(&rig, 0, ClipRef::whole(&src).unwrap(), 512);

     let mut out = vec![0.0f32; 8192];
     rig.e.render_into(&mut out); // prime: mounts and warm-up happen here
     ALLOCS.store(0, Ordering::Relaxed);
     ```
   - **Wrong behaviour:** the test would still pass if the park-at-EOF loop in `stream.rs:176-185` were deleted — i.e. if the 256 KiB ring were freed on the render thread again. The invariant is currently upheld by the code, not by this test.
   - **Evidence:** `crates/media/src/stream.rs:138-186` — the reader parks after `eof2.store(true, …)`, which is what makes the invariant true; nothing asserts it.
   - **Fix:** schedule the splice at a frame *inside* the measured window (e.g. `at_frame = 4096`, with the prime ending before that) so the retire is inside the measured region; add a second variant with a clip that reaches EOF before the splice, which was the original crash shape.
   - **Verification:** traced `Engine::render_into` → `render_block` → `render_chunk` → `PlaybackNode::render` (`stream.rs:355-449`) with `BLOCK = 512`; the fade window equals one block exactly, so the retire lands in the prime's first chunk. The engine-side twin (`crates/engine/tests/spike_a.rs:835`) primes a `clock_out` mount and schedules its unmount at frame 100 000 — never reached in the measured 8192 frames — so that one is honest about being steady-state only.

3. **The WAV reader rejects valid files that carry an odd-sized metadata chunk — RIFF word-alignment is not applied when skipping a chunk.**
   - **Severity:** MAJOR (a valid foreign WAV fails to import; untrusted-bytes parser)
   - **Location:** `crates/media/src/wav.rs:134-138`
   - **Trigger:** any RIFF file with a non-`fmt `/`data` chunk of odd size before `data` (a `LIST`, `cue `, `bext`, …):
     ```rust
     } else {
         reader
             .seek(SeekFrom::Current(size as i64))
             .map_err(|e| e.to_string())?;
     }
     ```
     RIFF pads odd-sized chunks to a word boundary, so after the seek the reader sits on the pad byte, reads `[pad, 'd','a','t']` as the next tag (≠ `b"data"`), treats the data chunk's own size as another skip, lands past EOF, and `parse_header` returns `Err("missing data chunk")`.
   - **Wrong behaviour:** a perfectly valid WAV from another tool is refused — `WavReader::open` errors, `Pool::list` files it in `errors`, `import` fails, and `WavWriter::recover`/`is_finalized` fail too. Silent data unavailability with no diagnostic about alignment.
   - **Evidence:** `wav.rs:81` reads the chunk size correctly; only the skip is missing the `size & 1` pad. Compare the writer, which never emits such a chunk, which is why CI cannot see it.
   - **Fix:** `reader.seek(SeekFrom::Current(size as i64 + (size & 1) as i64))`.
   - **Verification:** read `parse_header` (67-150) and every `wav.rs` test. The prior review listed this as a Minor; it was never fixed. This is also the one broken-code path I could not exercise directly (no `cargo` allowed), so the *fix* is certain but the specific "one chunk kind" naming is inference from the spec — the pad is what makes it fail either way.

4. **`docs/FIRST_SESSION.md`'s "Expected output" and its silence-detection snippet are both false against the current code.**
   - **Severity:** MAJOR (this is the doc `docs/beta-acceptance.md` and the README send a first-time user through, and a working run looks like a failure)
   - **Location:** `docs/FIRST_SESSION.md:57-66`, `:124-129`, `:22`, `:19`
   - **Trigger:** run the doc's own Step 1 script (`mount mixer channels=4` … `bounce 96000 /tmp/hello.wav`). The mixer's output port is unconditionally stereo (`crates/engine/src/plugins/mixer.rs:68-74`, `channels: 2`), the `Bounce` arm writes 16-bit PCM at `out_channels()` (`crates/host/src/lib.rs:1694-1697`), so the file is 96 000 × 2 × 2 + 44 = **384 044** bytes, not the documented 192 044. `summarize()` (`crates/host/src/lib.rs:4014-4022`) also prints a sixth line the doc omits:
     ```rust
     format!(
         "engine log events: {}\nmedia commands: {}\nunderruns: {}\ndeferred splices: {}\nmaster out node: {:?}\ndrain tail frames: {}{}\n",
     ```
     and the bounce logs one `Event::Arrangement` via `arrange_logged` (`lib.rs:1708`, `render.rs:653-661`), so the count is 9, not the documented 8. The diagnostic snippet then unpacks 4-byte floats from a 16-bit file:
     ```python
     print(sum(1 for i in range(44,len(d),4) if struct.unpack('<f',d[i:i+4])[0]!=0.0),'nonzero')
     ```
   - **Wrong behaviour:** a correct run disagrees with the documented output on three counts, and the Step 4C "silent-but-valid" check reports garbage instead of a nonzero count — the reader is `v as f32 / 32768.0` (`wav.rs:269`), the writer is 16-bit.
   - **Evidence:** as quoted; `crates/media/tests/phase1.rs:80-86` independently confirms the bounce is stereo (it reads "channel 0 (L) of the stereo bounce").
   - **Fix:** re-run the script, paste the real output, and change the snippet to `struct.unpack('<h', …)`; also fix the `../README.md#dev-environment` anchor (README's heading is `## Development`) and `~56 core tests` (engine has 89 `#[test]`s without the `fundsp` feature).
   - **Verification:** traced the whole Step 1 path (`parse_script` → `HostSession::from_script` → `apply` → `render_with_drain` → `Bounce`); `validate_patch` (`render.rs:340`) does produce the message the doc quotes in Step 4B, and `master.gain` range `0.0..=2.0` accepts `0.8`, so the script itself is valid — only the documented output is wrong.

### MINOR

5. **`wav.rs` documents a narrower reader than the code implements.**
   - **Severity:** MINOR · **Location:** `crates/media/src/wav.rs:3-9` and `:152`
   - **Evidence:** `//! Reader: 16-bit PCM and 32-bit float, mono or stereo (stereo → channel 0; the // Phase-0 graph is mono).` vs `if bits != 16 && bits != 24 && bits != 32` (`:119`) and `const MAX_CHANNELS: u16 = (MAX_CHUNK_BYTES / 4) as u16` = 256 (`:30`).
   - **Trigger / Wrong behaviour:** a reader (or a future contributor) trusts the crate doc and assumes stereo 16/32-bit only; 24-bit and up to 256 channels work today but are undocumented. The struct doc at `:152` repeats it.
   - **Fix:** restate as "16/24-bit PCM and 32-bit float, 1…256 channels (`with_channel` picks one)".
   - **Verification:** `a_twenty_four_bit_wav_reads_back` and `a_six_channel_file_reads_every_channel` (`wav.rs:631`, `:821`) both exercise what the doc denies.

6. **`resample.rs` states a 64-tap kernel and ≈ −80 dB stopband; the code uses 128 taps and no test proves −80 dB.**
   - **Severity:** MINOR · **Location:** `crates/media/src/resample.rs:12-13` vs `:36`
   - **Evidence:** `//! … this is a music tool. A 64-tap Kaiser-windowed sinc is ≈ -80 dB // stopband with a flat passband to ~20 kHz, for a cost paid once per import.` vs `const TAPS: usize = 128;` (the very next paragraph correctly says `[`TAPS`]-tap`).
   - **Trigger / Wrong behaviour:** the "tested property" figure has no test behind it. `the_passband_is_flat_and_distortion_free` asserts `< -60 dB` at 1 kHz and `< -40 dB` at 20 kHz; `downsampling_removes_content_above_the_new_nyquist` asserts `ratio < 0.05` (≈ −26 dB). Nothing pins −80 dB, and the tap count a reader would quote is wrong by 2×.
   - **Fix:** say 128-tap; either measure and pin the stopband or drop the figure.
   - **Verification:** read all 12 `resample.rs` tests.

7. **`PlaybackNode.pending`'s "Bounded command buffer" comment is false — nothing bounds it, and a text script can exceed 8 queued splices before the first render.**
   - **Severity:** MINOR (false in-code claim + a render-stack reallocation)
   - **Location:** `crates/media/src/stream.rs:318-322`, `:355-361`; reachable via `crates/host/src/lib.rs:1523-1530`
   - **Evidence:**
     ```rust
     // Bounded command buffer: pushes beyond the preallocated capacity
     // would allocate on the render path (a documented Phase-0 edge —
     // the profile schedules splices on clean boundaries, so bursts
     // beyond 8 are not expected).
     pending: VecDeque::with_capacity(8),
     ```
     `drain_mailbox` pops the whole (unbounded `Arc<Mutex<VecDeque<SpliceCmd>>>`) mailbox into `pending` with no cap.
   - **Trigger:** a `host v1` script of `play /a.wav ch0` followed by **nine** `splice` lines and one `bounce`. `HostSession::from_script` (`lib.rs:2327`) executes commands without rendering between them, so all nine are queued; the first `PlaybackNode::render` inside the bounce reallocs `pending` on the render stack.
   - **Wrong behaviour:** the comment's central word is untrue, and the edge the previous review flagged is reachable from the documented wire format with no test and no counting-allocator coverage.
   - **Fix:** `pending: VecDeque::with_capacity(64)` plus a documented refusal above it, or bound the mailbox (`mailbox().lock().len() < N`) at push time on the control side; delete the word "Bounded" either way.
   - **Verification:** read `stream.rs:232-362` and the host's `Splice` arm; confirmed `run_script`/`from_script` render only on `Bounce`/scheduled frames. The prior review's Minor ("bound the mailbox or pre-reserve") was re-documented, not fixed.

8. **`docs/capabilities.md` credits key bindings to `loop` and `chop` (neither has one) and over-counts the TUI's tests.**
   - **Severity:** MINOR · **Location:** `docs/capabilities.md:23`
   - **Evidence:** `| **Arrange**: cut, copy, cut/copy-paste, append, move, trim, loop, chop, fade, gain | keys \`x v y c p P d < > t H L J K g G f F\` | the TUI's 55 tests, …`
   - **Trigger / Wrong behaviour:** a user follows the "Where it lives" column for loop/chop and finds no key — `grep -i 'loop\|chop' crates/workflow/src/lib.rs` returns nothing, and neither does the TUI or iced source. Both ops exist only as `:` lines (`arrange loop_region …`, `arrange chop …`). Separately, `grep -c '#\[test\]'` over `spikes/tui-shell/src/{main,mixer,timeline}.rs` = 43 + 3 + 8 = **54**, not 55.
   - **Fix:** split loop/chop onto their own row citing `:` lines; correct the count (and add a "counts" check to the freshness routine so it cannot rot again).
   - **Verification:** dumped the full `KEYMAP` (`workflow/src/lib.rs:377-560`) and grepped both shells.

9. **The notes verifier only walks `.agents/notes/`, so the one broken cross-reference in the repo is ungated — and the same is true of dead rustdoc links.**
   - **Severity:** MINOR · **Location:** `scripts/verify-agent-notes.mjs:44-50`; broken link at `research/architecture/2026-08-20-ffmpeg-design-knowledge.md:103`; dead doc link at `crates/media/src/snap.rs:13`
   - **Evidence:** the verifier iterates only `LIFECYCLES = ['proposed','implemented','rejected']` under `.agents/notes/`. I ran the same link check over every tracked `.md` in the repo; exactly one link is broken:
     `research/architecture/2026-08-20-ffmpeg-design-knowledge.md:103 -> ../../.agents/notes/proposed/feature/2026-08-20-drain-eof-phase.md` (the note now lives at `implemented/feature/`). And `snap.rs:13` links `[`TempoMap`](../../engine/clock/struct.TempoMap.html)`, a relative HTML path that resolves to nothing on disk; no `cargo doc` job exists, so intra-doc links are never checked at all.
   - **Trigger / Wrong behaviour:** a note is promoted `proposed → implemented` and the links *into* it from `README.md`, `docs/`, `RESEARCH.md` and `research/` rot silently — the exact failure the gate's own comment (`verify-agent-notes.mjs:110-113`) says it exists to prevent, in the one direction it does not cover.
   - **Fix:** extend the verifier to walk `README.md`, `docs/**/*.md`, `RESEARCH.md` and `research/**/*.md` for *inbound* targets (repo-root-relative resolution, same external-reference exemptions), and add `cargo doc --no-deps --workspace` with `RUSTDOCFLAGS="-D rustdoc::broken_intra_doc_links"` to the `rust` job.
   - **Verification:** ran the link sweep read-only over all tracked markdown (one broken, listed above); confirmed no `cargo doc` step in `.github/workflows/ci.yml`.

10. **`text_format_arrange_ops_parse` asserts arity only, and its expected-op field is literally unused — while the comment claims "parses to the expected command".**
    - **Severity:** MINOR · **Location:** `crates/host/tests/arranger_commands.rs:555-575`
    - **Evidence:**
      ```rust
      // each arrange op's text grammar parses to the expected command (no silent
      // leniency — the wire schema must be strict about operands).
      for (line, _) in [
          ("arrange add_track t0 @0", 0),
          ("arrange remove_track t1 @0", 0),
          …
      ] {
          let cmds = host::parse_script(&format!("host v1\n{line}\n")).unwrap();
          assert_eq!(cmds.len(), 1, "{line}");
      }
      ```
      The tuple's second element is `0` for every row and destructured as `_` — the expected variant was dropped.
    - **Trigger / Wrong behaviour:** a copy-paste match arm in `parse_arrange` that mapped, say, `razor_split` to `Trim` would pass this test unchanged.
    - **Fix:** carry the expected `ArrangeOp` (or a discriminant string) in the tuple and assert with `matches!`.
    - **Verification:** checked the compensating coverage — `the_text_form_round_trips_every_state_command` (`crates/host/src/lib.rs:4859-5092`) does `format_command` → `parse_script` → `assert_eq!` over **every** `ArrangeOp`, so op *identity* is genuinely pinned elsewhere. That is why this is MINOR, not MAJOR: the weakened assertion is redundant, not the only guard.

11. **The workflow's vocabulary test covers only the 13 `arrange` ops; the shells' other log-op strings are hardcoded and outside it.**
    - **Severity:** MINOR · **Location:** `crates/workflow/src/lib.rs:308-331` + `:754-798`; unguarded strings at `spikes/tui-shell/src/main.rs:1376`, `:1387`, `:1444`
    - **Evidence:** `command_name()` returns `Some` only for arrange ops; `Action::StretchToTempo`, `ExportMix`, `MarkerSet`, `MarkerSeek`, `ClipRename`, `Undo`, `Redo` all fall through `_ => None`, so `every_log_op_names_a_real_host_operation` never samples them. The TUI builds those lines by hand:
      ```rust
      let prefill = format!("export {} f32", path.display());
      let prefill = format!("arrange set_marker {} ", self.snap_frame(self.snap.frame));
      let prefill = format!("arrange rename_clip {track} {} ", clip.id);
      ```
    - **Trigger / Wrong behaviour:** renaming any of those ops in the parser leaves the README's "test proving every action claimed to be a log op is an op the host's parser accepts" (`README.md:82`) technically true but vacuous for half the log vocabulary; only a manual `pty-check` run would surface it. (`rename_clip`/`set_marker` do happen to be exercised end-to-end by `spikes/tui-shell/src/main.rs:3729-3732`; `export` and `source_tempo` are not.)
    - **Fix:** extend `command_name()` to `Some("export")`, `Some("set_marker")`, `Some("rename_clip")`, `Some("source_tempo")`, `Some("stretch")`, `Some("undo")`, `Some("redo")` with sample lines, and route the TUI's prefills through one formatter instead of five `format!`s.
    - **Verification:** read the full `Action` enum and `command_name`; grepped both shells for hardcoded op names.

12. **`PlaybackNode::deferred`'s increment path has no test — every assertion in the repo is `== 0`.**
    - **Severity:** MINOR · **Location:** `crates/media/src/stream.rs:337-341`, `:394-396`; only assertion at `crates/media/src/stream.rs:573`
    - **Evidence:** `assert_eq!(node.deferred(), 0, "nothing deferred")` and `crates/media/tests/spike_b.rs:230-233` are the only uses; `grep -rn 'deferred'` finds no `> 0` assertion anywhere.
    - **Trigger / Wrong behaviour:** a splice whose `at_frame` has already passed (documented as a missed `try_lock`, or a command issued while the clock is past it) increments the counter. Nothing tests that path, so a regression that stopped counting — or that started deferring splices unnoticed — is invisible.
    - **Fix:** one test that pushes a `SpliceCmd` with `at_frame` in the past and asserts `deferred() == 1` and that the audio is still correct.
    - **Verification:** carried over unfixed from the prior review's test-coverage list.

13. **Post-open cpal stream errors are still never surfaced — the error-callback `Arc` is dropped when `open_output`/`open_input` return.**
    - **Severity:** MINOR · **Location:** `crates/media/src/devices.rs:119-123` + `:138-148`, `:308-312` + `:327-335`
    - **Evidence:**
      ```rust
      let err: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
      let err_cb = {
          let err = err.clone();
          move |e: cpal::Error| *err.lock().unwrap() = Some(format!("output stream: {e}"))
      };
      ```
      Only the pre-`play` error is drained (`:138-140`); `OutputHandle`/`InputHandle` carry `underruns`/`overruns` but no error slot, so a device that dies mid-session is silent.
    - **Trigger / Wrong behaviour:** ALSA drops the stream (device unplugged, xrun) after open → the callback keeps firing into a dead stream, or the user hears nothing with no message and no counter.
    - **Fix:** put the `Arc<Mutex<Option<String>>>` on the handles (as `underruns` already is) and drain it in the pump; a `last_error()` accessor plus a status line closes it.
    - **Verification:** prior-review Minor, unfixed; read both constructors and the two handle structs.

14. **`README.md`'s `cargo test -p media -- --ignored` is labelled "hardware capture" but runs a 25-minute soak that writes ~350 MB.**
    - **Severity:** MINOR · **Location:** `README.md:31` (repeated at `docs/FIRST_SESSION.md:143-144`)
    - **Trigger / Wrong behaviour:** a newcomer follows the line literally. Six `#[ignore]`d tests match: `hardware_input`, `spike_b::soak_25_minutes…`, `spike_b::devices_open_and_run`, `p1_2::hardware_capture_lands_pool_sources`, `midi::midi_out_real_port_sends_a_few_ticks`, `devices::output_device_report`, and `resample::throughput`. The soak's own ignore string gives the precise command.
    - **Fix:** `cargo test -p media -- --ignored capture` (and document each filter), or list the two capture tests explicitly.
    - **Verification:** enumerated every `#[ignore]` in `crates/media` via grep; read `spike_b.rs:536`'s reason string.

15. **The one script cited as the verification for the entire terminal path is run by nothing.**
    - **Severity:** MINOR · **Location:** `docs/tui-first-session.md:235-238`; script at `spikes/tui-shell/scripts/pty-check.sh`
    - **Evidence:** `is re-checked by \`spikes/tui-shell/scripts/pty-check.sh\`, which drives these keys through a real pty.` The CI `spikes` job runs `fmt`/`clippy`/`test` in the spike directory — no `pty-check` step — and `.githooks/pre-commit` does not mention it. `research/architecture/2026-08-24-…-gate-d1…` and `-d2…` both record "pty-check.sh not run" as a standing coverage caveat.
    - **Wrong behaviour:** a stale TUI doc or a broken real-terminal path passes every gate; the doc's freshness banner is the only defence and it is manual.
    - **Fix:** add a `needs: [libasound2-dev, util-linux, python3]` step to the `spikes` matrix that runs `scripts/pty-check.sh` (it already skips gracefully without a device), or state plainly in the doc that it is manual-only.

### NIT

16. `crates/host/src/rig.rs:123` — `assert!(source_kind("alsa").is_ok());` asserts only that the call did not fail; the returned `&str` is never compared. Neighbouring lines (116-122) do check values. Use `assert_eq!(source_kind("alsa").unwrap(), "alsa")`.
17. `crates/media/src/lib.rs:8-15` — the `//! Modules:` list names 7 of the 16 `pub mod`s (missing `arranger`, `capture`, `clip_editor`, `dither`, `midi`, `peaks`, `pool`, `snap`, `stretch`, `timeline`), and the same doc still says the master out is "owned by the mixer plugin" now that a `master` plugin can own it.
18. `crates/media/Cargo.toml:19` — `midir = { git = "https://github.com/Boddlnagg/midir" }` is a dependency on a personal fork. All three `Cargo.lock`s pin `8d00b6f6…`, so CI is reproducible, but the dependency dies with the fork; `RESEARCH.md:375` also still describes `midir` as "MIDI **input** (later: knobs/faders)" when `crates/media/src/midi.rs` uses it for output.
19. `crates/media/src/capture.rs:2-3` and `crates/media/src/devices.rs:85-86` embed gear characterizations ("the Soundcraft Notepad-12FX's 4 USB capture channels, the Scarlett 2i2's 2") directly in the crate, against `AGENTS.md:17-19` ("USB characterizations … belong in `music-composition-theory/studio/instruments/<name>/`"). A link would satisfy both.
20. `crates/media/src/resample.rs:501` is the only `#[ignore]` in the tree without a `reason = "…"` string (every other one names its run command), so `cargo test -- --ignored` gives no hint about it.
21. `.githooks/pre-commit:31-34` — the hook claims gates "warn and skip when its tool is missing", but a *present, too-old* `node` (the script needs ≥ 20.11 for `import.meta.dirname`; CI pins 24) aborts the commit with a raw `SyntaxError` under `set -e` instead of warning. Add a version probe alongside the `command -v node` check.
22. `Pool::recover` — the crash-recovery pass `crates/media/src/pool.rs:3-4` presents as part of what the pool *is* — has no caller outside its own tests (the host uses `open`/`conform`/`list`/`write_source` at `crates/host/src/lib.rs:741,744,766,1130`; neither shell calls it). A crashed take is still readable because `WavReader::open` clamps to the file length, so the impact is a permanently-`finalized: false` index entry rather than lost audio — but the doc presents recovery as live.

### Test gaps

- **No property/fuzz coverage for either untrusted-bytes parser, and no property-test dependency anywhere.** `grep -rn dev-dependencies --include=Cargo.toml` returns nothing in any crate; there is no `proptest`, `arbitrary`, `quickcheck`, or `fuzz/` target in the repo. The two parsers that eat bytes from disk or from a wire are covered by hand-picked cases only: `crates/host/tests/parse_script_robustness.rs` (4 tests, 16 hardcoded truncated lines) and `crates/media/src/wav.rs` (`rejects_garbage` is a single 21-byte non-RIFF string). The project's own research already records this as a requirement — `research/architecture/2026-08-20-ffmpeg-design-knowledge.md:105` lists "**Fuzz the media-pool import path from day one**" as a recommendation. Minimum: a `proptest` that `parse_script` never panics for arbitrary `&str`, and one that `WavReader::open` + `read_into` never panics for arbitrary byte vectors and always returns frames consistent with `total_frames`.
- **WAV edge cases the tests cannot see:** odd-sized metadata chunks (finding 3), a `fmt ` chunk with `size < 16` or `size > 40`, `data` before `fmt`, a `data` size past EOF (only the crashed-take path touches the clamp), `seek_frames` at `total_frames`, a zero-length `read_into` buffer, and 24-bit recovery (finding 1).
- **The deferred-splice counter's increment path** (finding 12) — every assertion is `== 0`.
- **`PlaybackNode.pending` past 8** (finding 7) — no test issues more than one queued splice, and the media counting-allocator test never sees a retire inside its measured window (finding 2).
- **The `fundsp` feature is never compiled or tested in CI.** `crates/engine/Cargo.toml:11-15` gates `plugins::fundsp_synth` and `crates/engine/tests/fundsp_synth.rs` (9 tests) behind `--features fundsp`; `ci.yml:60` runs plain `cargo test --workspace --locked`, so 202 lines of DSP source and 9 tests are dead to every gate. Cheap fix: one extra CI step, or a `required-features` job.
- **The whole manual acceptance surface is un-gated.** `docs/beta-acceptance.md` and `docs/tui-first-session.md` are the two "second person" entry points, and neither is executed by CI or a hook (finding 15 covers the one script that could be).
- **`crates/shell/src-tauri/src/lib.rs` holds 5 `#[test]`s that no gate can run** — correct given the retirement, but worth stating in the crate's own docs so nobody reads them as live coverage.

### Design risks

- **The render path is not the audio thread in the current architecture, but the invariant is written as if it were.** The cpal callback only pops an SPSC ring (`devices.rs:271-292`); rendering happens on a control or pump thread (`live.rs`). Both counting-allocator proofs measure the *render call stack*, not the callback. This is not wrong today, but it means findings 2 and 7 are "allocation on the render stack", not "glitch on the device" — and the tests' language ("the audio path") invites the stronger reading.
- **Two `Result<T, String>` error surfaces cross the shell boundary** (`live.rs` snapshot, the TUI status line). A shell matching on error substrings will break when a message is reworded; the prior review already flagged revisiting `thiserror` at the IPC boundary.
- **`verify-agent-notes.mjs` is single-direction** (notes → targets), so the tree can rot from the outside in (finding 9). The notes' own convention ("a later decision overrides an earlier one, link rather than rewrite") means promotions *will* happen; today nothing catches a stale inbound link.
- **The git dependency on a personal fork** (finding 18) is the single largest availability risk in the build graph, and it is invisible in the lock file's shape.
- **`_pad` false sharing in `ring.rs` and the unbounded mailbox** were both prior-review Minors that were re-documented rather than fixed; the pattern (a known edge gets a comment instead of a bound) is worth watching.

### Checked and clean

- **Prior-review criticals, re-verified as genuinely fixed:** `fill_output` pops per *frame* with content assertions (`devices.rs:259-293`, tests at `:393,407,421,437,453,469` — a stronger fix than the review's ask, since it became unit tests rather than a hardware test); the reader parks at EOF holding its `Arc` clones (`stream.rs:176-185`); the >4 GiB WAV guard is `Err`-based with the odd-data pad accounted for (`wav.rs:505-522`, test `:774`); `parse_script` uses `word()`/`exact()`/`port_ref().get()` throughout with no direct `words[N]` indexing reachable (`lib.rs:3196-3574`); `render()` returns `Result` and mixer-unmount clears `pending_cords`/`player_mailbox`/`mixer_channels` (`lib.rs:1401-1405`, `:1854-1859`); the text form expresses `pool`/`arrange`; `mount` params are finiteness-checked (`render.rs:229-235`).
- **`parse_script` error paths:** every arm is arity-checked via `exact()`; `exact`'s `words.len() - 1` cannot underflow because the `kind` match guarantees `len ≥ 1`; the `mount` arm's `&words[2..]` is guarded by the `word(&words, 1, …)?` that precedes it; `parse_snap`'s `token[5..]` is always on a char boundary; `snap=` on a frameless `arrange` op is refused (`lib.rs:3665-3669`), so the outer `snap_used` cannot mask a silent no-op.
- **Format/parse round-trip:** `the_text_form_round_trips_every_state_command` (`lib.rs:4859-5092`) formats every `ArrangeOp` and command variant, re-parses, and compares — including the `rename_clip` empty-name 3-word form and the "pure actions have no text form" list.
- **Media determinism and the reflog invariant:** `full_script_bounces_byte_identically` (bytes, logs, underruns, deferred, splice-vs-control diff), `refused_commands_are_rejected_identically`, `a_refused_media_command_is_never_logged`, `bounce_is_byte_identical_on_replay`, `save_and_load_round_trips_a_session`, `a_warm_seek_equals_a_replay` — all real-content assertions, no timing luck.
- **Devices unit tests** (`fill_output`/`fill_input`): partial-frame starvation, zero-channel degradation, overrun counting, and the config-negotiation preference rules — precise value assertions, no vacuous `is_ok()`.
- **Pool tests** (`crates/media/tests/pool.rs`, 18 tests): the split/import/conform/backup-naming matrix, torn-sibling re-derivation, narrower-import replacement, and the guard cases including the whitespace id.
- **Workflow keymap invariants** (`workflow/src/lib.rs:678-866`): `no_key_is_bound_twice`, `every_listed_key_resolves_to_its_action`, `the_help_spelling_contains_the_keys_it_names`, `every_action_has_a_name`, and the bidirectional vocabulary test.
- **CI (`ci.yml`):** does run everything `AGENTS.md:37` claims — `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`, `node scripts/verify-agent-notes.mjs`, and a `fail-fast: false` matrix job per shell spike (both `spikes/*` workspaces are covered; `--locked` is used everywhere and all three lockfiles are tracked). The `spikes` job runs fmt/clippy/test *in the spike directory*, so the pre-commit hook (root-only `cargo fmt --all --check`) is strictly weaker but not contradictory — the workflow's own framing ("CI is the authority") is accurate. `permissions: contents: read` and the `push: [main]` / `pull_request` split match their stated rationale.
- **`.githooks/`:** `pre-commit` orders fmt → archived-guard → notes verifier and degrades to warnings when a tool is absent (with the Node-version caveat in finding 21); `commit-msg` is idempotent, appends exactly one `Assisted-by` trailer, and deliberately leaves retired identities anonymous.
- **`crates/shell` retirement:** consistent everywhere — excluded in `Cargo.toml:8`, never built in `ci.yml:15-16`, `RETIRED.md`'s two documented commands both work (`src-tauri/Cargo.toml` is its own workspace; `package.json` + `pnpm-lock.yaml` are present), and 35 tracked files remain as the stated porting reference.
- **Docs cross-checked and correct:** `docs/beta-acceptance.md` keys (`x u Ctrl+r y p P a R J D b [ ] V U i E T ' ; " C W X`) all match `KEYMAP`; `--wave`/`--script` flags (`tui-shell/src/main.rs:99-100`), `session.txt`/`journal.txt` (`lib.rs:2749,2751`), and `master ceiling/threshold/ratio` (`HOST_PARAMS` at `lib.rs:114-121`, `master.rs:65-90`) all exist; `docs/tui-first-session.md`'s dither, tail, and refusal messages match the implementation; `RESEARCH.md`'s `DITHER_SEED` description (first 64 bits of √2) is correct.
- **Grep-verifiable "no match" claims I could not fault:** `stretch.rs`'s "2048 frames ≈ 43 ms at the default hop" (window = 2 × `DEFAULT_HOP` = 2048 ✓), `peaks.rs`'s 256-sample base bin (`PEAK_BASE_BIN = 256` ✓), `dither.rs`'s `q / 32767` grid (`dither.rs:76-79` ✓), `MAX_DRAIN_FRAMES = 48_000 * 60` and the ~46-minute / 1 GiB bounce bound (`render.rs:107`, `lib.rs:1890-1903` — arithmetic checks out), `SEEK_WARMUP_FRAMES = 48_000` matching the "one-second warm-up" claim, and `MAX_PDC` having been raised from 64 to 4096 (prior major #8, fixed).

---

## 2. Adversarial verification (verbatim)

I'll start by reading the actual code each claim names.

## Verdicts

### `PARTLY 6-tests-ci-docs#4`

Two independent halves; one holds, one does not.

**Holds — the "Expected output" block is false in three concrete ways.**

- *Byte count.* `crates/host/src/lib.rs:1694-1696` writes at `graph.out_channels()`, and the mixer's audio-out port is unconditionally `channels: 2` (`crates/engine/src/plugins/mixer.rs:68-74`). I traced the tail: the last euclid trigger is step 14 → frame 84000, `blip_len=1800` → ends 85800 < 96000, so `ToneGen::has_tail` is false at the end and `flush_frames()` is 0 (every node's `latency()` is 0) → the drain contributes 0 frames. Output = 96000 × 2 ch × 2 B + 44 = **384044**, not 192044. Corroborated in-repo: `.agents/notes/proposed/architecture/2026-09-10-hub-and-spokes-and-the-llm-seam.md:138` records `host: bounced 384044 bytes`.
- *Event count.* `lib.rs:1707-1708` → `arrange_logged` → `render.rs:619-627` pushes `Event::Arrangement`; `event_count()` = `log.len()`. 4 mounts + 3 patches + 1 set_param + 1 `MediaBounce` = **9**, and `docs/FIRST_SESSION.md:79`'s own arithmetic ("four mounts, three patches, one param change") omits the bounce record.
- *Missing line.* `summarize` (`lib.rs:4013-4022`) prints six lines; the doc block shows five. `drain tail frames: 0` is absent.

**Does not hold — the snippet's stated consequence.** `struct.unpack('<f', …)` on a 16-bit file is indeed a dtype mismatch, but in the documented Step 4C scenario it does **not** "report garbage instead of a nonzero count", for a reason the reviewer missed: `NoteEvent::duration` is **write-only**. `ScaleGen` fills it from `note_len` (`graph.rs:542-547`), and `ToneGen` ignores it, using its own `blip_len` instead (`graph.rs:624`: `self.schedule(note.offset, self.blip_len, freq)`). `grep -rn "\.duration" --include='*.rs' .` returns nothing. So `note_len=0.25` does **not** silence the output — the bounce is byte-identical to Step 1 and the snippet prints a large nonzero count. The doc's *premise* is what's broken, not the snippet's verdict (and the f32 misread does not flip a nonzero answer either way).

The two smaller fix notes in the claim are correct: `README.md:118` is `## Development` (anchor `#dev-environment` is dead), and `cargo test -p engine` runs 33 src + 65 integration − 9 `fundsp_synth` = **89** tests, not ~56.

---

### `CONFIRMED 6-tests-ci-docs#1`

`crates/media/src/wav.rs:409` passes `h.bits == 32` as the `float` flag into `data_bytes` (`wav.rs:505-506`), so a 24-bit file gets `bytes_per_sample = 2` while `h.block_align()` (`wav.rs:47-49`) correctly used 3. `frames` is right, `data_bytes` is 2/3 of the real byte count, and `wav.rs:422` `f.set_len(h.data_offset + data_bytes)` truncates. `recover` returns `Ok(frames)` with the *pre-truncation* count, and `pool.rs:254` files it under `report.finalized`.

Trigger I constructed (simpler than the one given, and it needs no pool): hand-write a 24-bit mono WAV (the shape `wav.rs:792-815 write_pcm24` already builds) with 5 frames → 15 audio bytes, `data_offset` 44, file_len 59. `WavWriter::recover(&path)` returns `Ok(5)`, `set_len(54)` drops 5 bytes, and `WavReader::open` then reports `total_frames() == 3`. A third of the audio gone, no error.

The pool path also works as claimed: a 24-bit mono file with an odd frame count **plus** the RIFF pad byte gives `is_finalized == false` (`wav.rs:435`), so `pool.rs:252-253` calls `recover` on it; and `Pool::import` byte-copies such a file verbatim when the rate already matches (`pool.rs:479-485`), so 24-bit PCM really can sit in a pool. No test covers a 24-bit recover — the four call sites are `wav.rs:715` (16-bit), `wav.rs:767` (float), `spike_b.rs:371` (16-bit), `pool.rs:74` (float via `create_float`). The reachability caveat (no product caller of `Pool::recover`; grep confirms only tests) is accurate.

---

### `CONFIRMED 6-tests-ci-docs#2`

The timing trace is exact. `schedule_splice(&rig, 0, …)` (`spike_b.rs:510`) puts the command in the mailbox *before* the prime. `render_into` chunks 8192 at `BLOCK * ch` = 512 (`graph.rs:27`; the rig's out node is the 1-channel `PlaybackNode`, `spike_b.rs:88-95`). In the prime's **first** block: `drain_mailbox` (`stream.rs:355-361`) moves it to `pending`; `pop_front_if(|c| c.at_frame < f1)` with `0 < 512` fires it (`stream.rs:391-406`) with `offset = 0`, `crossfade = 512`; the 512-sample fade retires the old `Player` at `i = 511` (`stream.rs:439-442`) → `FilePlayer::drop` (`stream.rs:222-230`) runs on the render thread, all before `ALLOCS.store(0, …)` at `spike_b.rs:514`. The measured window (second `render_into`, 8192 frames) has an empty mailbox, empty `pending`, `fade: None` — pure steady-state popping. The test's comment at `spike_b.rs:507-509` is false, and the park-at-EOF invariant (`stream.rs:175-185`) is asserted nowhere. The engine twin check is also right: `spike_a.rs:848` unmounts at frame 100 000, never reached in the measured 8704-frame window.

One correction to the proposed fix, which makes the gap *worse*, not better: the `CountingAllocator` counts only `alloc` and `realloc` — `dealloc` is uncounted (`spike_b.rs:482-484`, same at `spike_a.rs:821-823`, `phase1_mixer.rs:367`, `p1_2.rs:333`). A render-thread *free* of the 256 KiB ring is invisible to this harness no matter where the splice fires. Moving the splice to `at_frame = 4096` would only start covering the application path, not the free.

---

### `CONFIRMED 6-tests-ci-docs#3`

`crates/media/src/wav.rs:134-138` seeks `size` with no `+ (size & 1)`, and `wav.rs:81` reads the declared size correctly, so the misalignment is real. Traced on a file with `fmt `(16) then `LIST` of odd size 5 then `data`: after the skip the reader sits on the pad byte; the 8-byte read yields tag `[0x00,'d','a','t']` ≠ `b"data"`, so the else arm computes `size` from `['a', s0, s1, s2]` ≈ 1.6 GB, seeks past EOF, `read_exact` fails, `break`, and `wav.rs:142` returns `Err("missing data chunk")`. Consequence chain is accurate: `Pool::list` files it in `index.errors` (`pool.rs:208-213`), `Pool::import` errors (`pool.rs:392`), and `WavWriter::recover`/`is_finalized` both go through `parse_header`. No test writes any chunk other than `fmt `/`data` — the three hand-built helpers (`wav.rs:606`, `wav.rs:792`, `crates/media/tests/pool.rs:198`) all use a 16-byte fmt and no extra chunks.

One imprecision in the evidence, not in the finding: `LIST` and `cue ` are always even by construction (sub-chunks are word-aligned; `cue ` is `4 + 24n`), so they cannot trigger it. The realistic odd pre-`data` chunks are `id3 `, `iXML`, and BWF `bext` with an odd-length CodingHistory — `bext` is written *before* `data` by the BWF convention, so the class of trigger is real.

---

### New findings

**1. `WavWriter::recover` hard-codes a 44-byte header layout, but `parse_header` accepts 18- and 40-byte `fmt` chunks — so recovery writes into the wrong bytes and can never converge.**
`crates/media/src/wav.rs:33-34`
```rust
const RIFF_SIZE_POS: u64 = 4;
const DATA_SIZE_POS: u64 = 40;   // assumes a 16-byte fmt body
```
`wav.rs:416` writes `36u32 + data_bytes` (the 36 is `file_size - data` only for the canonical 44-byte header) and `wav.rs:418-420` patches the size at absolute offset 40 — while `wav.rs:84` (`let n = size.min(40)`) deliberately accepts a 40-byte fmt body, i.e. WAVEFORMATEXTENSIBLE, which is what essentially every 24-bit writer emits (and 18-byte WAVEFORMATEX).
Trigger: a torn/unfinalized extensible WAV (fmt size 40) in a pool dir → `Pool::recover`. The data tag sits at file offset 60 and its size field at 64; the 4-byte patch at 40 lands on the fmt body's `dwChannelMask` (body offset 20), the real size field keeps its `0xFFFFFFFF` placeholder, and `set_len` still truncates.
Consequence: the channel mask is silently replaced by a byte count, `is_finalized` (`wav.rs:435`) stays `false` forever, so every subsequent `Pool::recover` re-corrupts the file while reporting it as recovered. Distinct from claim 1 (arithmetic vs. offsets), and the proposed fix there — pass `(h.bits / 8) as u64` — does not address it.

**2. `Pool::recover` turns a trailing metadata chunk into audio in a perfectly valid 16-bit or float WAV.**
`crates/media/src/pool.rs:252-253` calls `WavWriter::recover` on any `.wav` where `is_finalized` is false, and a file with a trailing `LIST`/`id3 ` is false by definition (`wav.rs:435`). `wav.rs:403-404` then computes `actual_bytes = file_len - data_offset`, which **includes** the trailing chunk.
Concrete: 16-bit mono, 1000 frames (2000 B at [44, 2044)), then a 26-byte `LIST` chunk → file_len 2070. `recover` gets `actual_bytes = 2026`, `frames = 1013`, `data_bytes = 2026`, and `set_len(44 + 2026) = 2070` leaves the length untouched while the header now declares 1013 frames. Frames 1000-1012 are the ASCII of `LIST`+size+payload read as int16 samples (`'L'` → 0.59 full scale), and `is_finalized` then returns `true`, so the damage is permanent and `rebuild_peaks` (`pool.rs:267`) bakes peaks over the garbage. Claim 1 mentions trailing chunks only as a *trigger* and only derives the 24-bit consequence; this is the 16-bit/float consequence, and it fires on far more common material.

**3. `NoteEvent::duration` is write-only, so `scale.note_len` is a logged parameter nothing reads — and `docs/FIRST_SESSION.md`'s entire Step 4C rests on it.**
`crates/engine/src/graph.rs:44-49` declares `pub duration: u32`; `graph.rs:542-547` fills it from `note_len`; the only consumer, `ToneGen`, discards it:
```rust
// graph.rs:620-624
for note in io.notes_in {
    let freq = 440.0 * 2f32.powf(note.pitch / 12.0);
    self.schedule(note.offset, self.blip_len, freq);   // blip_len, not note.duration
}
```
`grep -rn "\.duration" --include='*.rs' .` (excluding `target/`) returns nothing.
Trigger: `docs/FIRST_SESSION.md:116-121` tells the reader to change `note_len=2400` to `note_len=0.25` and states "The bounce succeeds and produces **silence**". It cannot: `scale_factory` stores `0.25 as u32 = 0` into a field no node reads, and the tone's blip length comes from the separate `blip_len` mount param.
Consequence: the bounce is byte-identical to Step 1; the "silent-but-valid" diagnostic prints a large nonzero count; and the lesson the step exists to teach ("Length parameters are measured in sample frames, not seconds") is attached to a parameter that has no audible effect at all. Every shell's demo script (`spikes/tui-shell/src/main.rs:61`, `spikes/iced-shell/src/main.rs:61`) passes `note_len=2400` and is relying on a no-op.
