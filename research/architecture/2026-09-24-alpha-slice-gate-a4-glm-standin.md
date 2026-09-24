# Reviewer gate (stand-in) — alpha slice A4 (stereo material), GLM-5.3

> Research input, 2026-09-24. Co-worker: **GLM-5.3** (`zai/glm-5.3`), read-only, standing in for
> the designated gate **Kimi K3** (unavailable: `opencode-go/kimi-k3` accepted the brief and failed
> before finishing with no output — the same failure shape as A1–A3, see
> [`2026-09-23-kimi-gate-alpha-slices.md`](2026-09-23-kimi-gate-alpha-slices.md) and its A4 addendum).
> The owner's account reports the API's 5-hour window at 100 % (reset 2026-09-24 14:09 local) and
> suggests the **256 k-context** model name; the retry is scheduled after the reset.
>
> Scope: slice A4 (stereo material — one pool source per channel), working tree at the time
> (`crates/media/src/wav.rs`, `crates/media/src/pool.rs`, the TUI's `--wave`/`--probe`, the new tests,
> and the note `.agents/notes/implemented/feature/2026-09-24-stereo-material-per-channel-pool.md`).
> The reviewer was told to attack the claims, cite file:line, separate verified from inferred, and end
> with a verdict.
>
> **Disposition: `merge with changes`, and every finding was fixed or answered before merge.** Six of
> the seven held up in code; the seventh (a documentation number) was wrong in the *note*, not the
> code. None were caught by the tests.
>
> 1. **Replacement stopped at the whole-file source** (should-fix) — a stereo file imported over a
>    stem that held a 5.1 take left `jam.ch2`…`jam.ch5` in the pool, so a clip on them played
>    replaced audio. Fixed: `replace_sources(id, keep_channels)` drops the old `{id}.wav` **and every
>    `{id}.ch{k}` at or beyond the new channel count**. Regression test
>    `a_narrower_import_removes_the_older_channels`.
> 2. **Siblings were not crash-safe, and a torn sibling was trusted** (should-fix) — channels were
>    written straight to their final names, so an interrupted split left an un-finalized take that
>    `Pool::recover` would "finalize" into a truncated channel, and `expand_one` skipped any sibling
>    that merely existed. Fixed: **every write is staged** (`{dest}.converting`) and renamed only
>    once all channels exist, and siblings are now **always re-derived** (the naming makes
>    `{id}.ch{k}` mean "channel *k* of `{id}`", so overwriting is the correct reading of the name).
>    Regression test `a_torn_sibling_is_re_derived_not_trusted`.
> 3. **"A failed split leaves the pool as it was" was false past the first channel** (should-fix) —
>    a failure on channel 2 of 6 left channels 0–1 in place beside the untouched `{id}.wav`. Fixed by
>    the same staging: nothing is committed until every channel is rendered, and a failure removes
>    the staged temporaries.
> 4. **An unrelated `{id}.ch{k}` was silently accepted as channel *k*** (note) — the `is_file()` skip
>    made a hand-placed file of the same name stand in for a channel. Resolved by the same decision as
>    2: the name means the channel, so the pass re-derives it rather than trusting it.
> 5. **A same-file multi-channel `Import` under-reported** (note) — it returned one `Conform` for
>    `{id}`, so `Import::ids()` was not per-channel and the shell mounted a single unpanned track for
>    a stereo file that already lived in the pool. Fixed: `expand_one` returns **one entry per pool
>    source**, the file's own first (carrying `extracted`); `Import::ids()` is now always the channels.
> 6. **Documentation drift on the channel ceiling** (note) — the note and the plan said 512; the code
>    refuses above `MAX_CHANNELS` = 256 (a frame must fit the reader's 1024-byte chunk). The note's
>    rationale ("wider than the WAV block-align field") was also not what the code does — the declared
>    block-align is ignored and re-derived. Both corrected in the note, the plan, the media-engine
>    note, and the `parse_header` comment.
> 7. **A pre-existing `{id}.wav.pre{rate}` backup was silently truncated** (note) — `hard_link` failed
>    with `EEXIST` and the `fs::copy` fallback overwrote the older preservation. Fixed:
>    `free_backup` numbers the name (`.pre2ch.2`, …). Regression test
>    `preserving_an_original_does_not_clobber_an_earlier_backup`.
>
> Findings 1–3 were the substantive ones: each was a **data-fidelity** bug in a path the tests did not
> exercise (a replace across channel counts, an interrupted split, an existing sibling), which is
> exactly what the gate exists to catch.
>
> Reviewer-verified claims (recorded because they are the slice's load-bearing ones): the channel
> ceiling is sound for every bit depth and cannot spin (`chunk_frames == 0` returns); `read_exact` is
> safe for a growing or torn file because `open` clamps `data_bytes` to the file length and floors
> `total_frames`; the reader stays stack-only and allocation-free; the host 440-left/880-right test is
> a real proof (a swap, a drop, or a centre-pan bug each fails it); the "mono at the session rate"
> invariant holds after conform on every path; `recover` never mutates a source.

---

## Verbatim review

Adversarial review of the A4 stereo-material change (read-only; all tests run and passing: media, host, tui-shell).

## Verdict
merge with changes

## Findings

1. should-fix — "Importing over an existing id replaces it" is false when the earlier import had MORE channels. `Pool::import` multi-channel path (crates/media/src/pool.rs:347-350) removes only `{id}.wav`/`{id}.peaks`; it never removes stale `{id}.ch{k}.wav` for k ≥ the new channel count. Scenario: import a 6-channel `jam.wav` (jam.ch0..jam.ch5), later import a stereo `jam.wav` → jam.ch2..jam.ch5 survive with the OLD material, still addressable by clips — exactly the staleness the removal was added to prevent. The test `a_stereo_import_replaces_the_mono_source_holding_that_id` only covers mono→stereo.

2. should-fix — sibling writes are not crash-safe, and `expand_one`'s idempotence check treats a torn sibling as complete. `write_channel` writes directly to the final `{id}.ch{k}.wav` name (pool.rs:488, and pool.rs:318 in import) — unlike `{id}` itself, which is written to `.converting` and renamed (pool.rs:520,544). A crash mid-sibling-write leaves an un-finalized partial file; the next `conform` hits `sibling.is_file()` → `continue` (pool.rs:485-487) and skips it; `Pool::recover` then finalizes the torn file, permanently presenting a truncated channel. The original stereo is preserved as `{id}.wav.pre2ch`, but nothing ever re-derives the sibling from it. The doc claim "a crash never leaves the pool without its source" (pool.rs:464-465) is only true for `{id}`.

3. should-fix — the comment "a failed split leaves the pool as it was" (pool.rs:345-346) is false for failures beyond the first channel. A `write_channel` failure on ch2 of 6 removes only ch2's file (pool.rs:337); ch0/ch1 (and their peaks) remain in the pool alongside the untouched `{id}.wav`. Likewise a `rebuild_peaks` failure returns Err with all channels-so-far left in place (pool.rs:320-321). Mixed old/new material under one stem after a partial failure.

4. note — id collision with a generated name is silently accepted. If a hand-filled pool already has an unrelated `jam.ch1.wav`, `expand_one` skips it as "already split" (pool.rs:485) and reports `extracted` without it; the unrelated file is then presented as jam's channel 1 (wrong material, no warning). Inherent to the `.ch{k}` naming, but a `channels`/frames sanity check on the existing sibling would catch most cases.

5. note — same-file multi-channel `Import` under-reports. `import`'s same_file branch (pool.rs:300-304) returns `sources = expand_one(...)` = one Conform for `{id}` only, so `Import::ids()` ≠ per-channel ids and the doc "sources.len() unless the import was refused" (pool.rs:105) is wrong there. Consequence in the shell: `--wave` on a stereo file that already lives in the session pool mounts ONE track (`sources.len() != 2` → no pan, tui main.rs wave_script), i.e. the right channel stays in the pool but out of the arrangement. Edge case (the shell's session pool is per-process), tested only as `importing_a_pool_source_over_itself_reports_it`.

6. note — doc inaccuracies: the note and task say the guard is "a frame wider than the WAV block-align field"; the code refuses `channels > MAX_CHANNELS` (256 — a frame wider than the reader's 1024-byte chunk, wav.rs:28-30,108). The declared block-align field is in fact ignored and re-derived (wav.rs:47-49) — safer, but the stated rationale isn't what's implemented. Also `--probe`'s `channel_count` is read from the snapshot (main.rs:1468 area) — fine.

7. note — `expand_one` can overwrite a pre-existing backup: if `{id}.wav.pre{rate}` already exists, `hard_link` fails with EEXIST and the `fs::copy` fallback (pool.rs:536-537) silently truncates the old backup.

## Verified
- Channel guard is sound for all bit depths: channels ≤ 256, so block_align ≤ 1024 = MAX_CHUNK_BYTES; `chunk_frames == 0` early return (wav.rs:247-249) makes non-progress impossible, and there's a test at the ceiling and above (wav.rs `a_frame_wider_than_the_chunk_is_refused`). The `pick` byte offset (`f*ba + pick*(bits/8)`, pick < channels) can't overrun the chunk.
- `read_exact` change is safe: `open` clamps `data_bytes` to file length and floors `total_frames` (wav.rs:170-176), so a torn/live take never spans a partial frame at open time; both `read` and `read_exact` see EOF identically on a growing file. Stack-only buffer, no allocation — the reader's invariants hold.
- Host end-to-end test is a real proof: distinct 440/880 tones plus per-output-channel zero-crossing counts (arranger_commands.rs `a_stereo_pool_source_plays_both_channels_on_the_right_sides`) — a swap, a drop, or a center-pan bug each fails (a mixed 440+880 channel has the wrong crossing count).
- conform filter/idempotence: `channels > 1` takes a source out after expansion; index snapshot prevents reprocessing new siblings; stereo-at-session-rate backup lands as `.pre2ch`, never indexed; `{id}` keeps meaning channel 0 (host test asserts it).
- capture writes mono `{take}.ch{k}` (capture.rs:42), `list` reports `channels`, `recover` never mutates sources — invariant "mono at session rate" is maintained on every path after conform, which the host runs on every pool adoption (host lib.rs:425).
- All three suites pass (media 29+13+77+… , host 14+…, tui-shell 29).

## Not verified
- Live `--probe`/`--wave` runs against real audio hardware (no `cargo run` performed).
- Concurrent import into the same pool from two processes (no locking anywhere — pre-existing).
- Filesystem-dependent behavior of the hard-link/copy/rename dance on non-ext4 filesystems.
