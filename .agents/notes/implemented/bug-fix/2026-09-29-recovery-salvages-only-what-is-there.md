# Agent Note: Recovery salvages what is there, and never grows a file

Status: implemented

## Problem

`is_finalized` decided "well-formed" by **exact equality**: `data_offset + data_bytes == file_len`
(`wav.rs`). That reads a crashed take as "the header declares more than the file holds", and it is
right there — but equality also makes it false for every file that holds **more** than it declares,
and holding more is *legal*:

- a foreign WAV with a trailing `LIST`/`INFO`/`fact`/`cue` chunk — what a DAW export, `ffmpeg` or
  Audacity writes routinely;
- the RIFF **pad byte** after an odd-length data chunk, which a conforming writer emits for
  odd-frame-count 24-bit material.

So `Pool::recover` (`pool.rs`) — which calls `WavWriter::recover` for any source where
`is_finalized` is false — ran its crash salvage over files this project never wrote. And the salvage
was the wrong shape for them: `recover`'s rescan measured the audio as *everything from `data_offset`
to EOF*, so the trailing chunk was counted as audio. For a 16-bit mono file of 1000 frames plus a
28-byte `LIST` chunk, that is 1014 frames declared over 2000 bytes of take; `set_len` could not cut
the file (there was nothing to cut, being the file's own tail region) and the header was patched to
declare the metadata as audio. The [media-pool
note](../feature/2026-08-24-p1-3-3-media-pool.md)'s stated invariant — "never mutates a well-formed
source" — did not hold for any WAV this crate did not write. Reported independently by the
[Space Bunny review](../../../../research/architecture/2026-09-29-space-bunny-review-2-media-io.md) as
the neighbour of the sample-width fix
([note](2026-09-29-recovery-uses-the-headers-sample-width.md), whose "Still open" section recorded it
and deliberately did not bundle it).

The hazard is the **absence** of a shape being read as the **presence** of a crash. "The declared
size is not the file length" is not evidence of an unfinished write: in one direction the bytes after
the data chunk are legal metadata, and in the other they are evidence the write died. Equality cannot
tell those apart, so it guesses, and the guess it makes is the destructive one.

## Decision

**Unfinalized now means one thing — the file is short of its own declaration** — and everything else
is left alone.

- **`is_finalized` is `data_offset + data_bytes <= file_len`.** `true` when the file holds at least
  every frame it declares. A trailing chunk and a pad byte both make it `true`, so the pool's crash
  pass never offers those files to `recover` at all. A take that is genuinely short — the
  `0xFFFF_FFFF` placeholder `write_header` puts up front, or a header patched for more audio than the
  write reached — is `false`, exactly as before. The predicate is one comparison, and the direction of
  the inequality is the whole decision.
- **`recover` refuses a file whose data chunk is not the file's tail** (`declared_end < file_len`),
  returning `Err` and touching no byte — the conservative refusal over a guess. A direct caller
  cannot re-introduce the inflation the pool no longer attempts. This is deliberately a *refusal* and
  not a silent success: the alternative (recover the declared audio and re-append the trailing bytes)
  means moving another tool's bytes around to preserve a file we did not write and are not sure of.
- **The salvage is capped by the bytes the file holds**, `frames = min(held, declared) / block_align`,
  so `set_len` is structurally a shortening: `min` then floor division cannot produce a size past
  `file_len`, and a `debug_assert!` names that invariant where the patch is written. Recovery may cut
  a torn partial frame at a frame boundary; it cannot extend a take past what it declared.

## Alternatives considered

- **Keep equality, and teach `recover` to find the data chunk's real end** (walk the chunks, treat
  everything after `data` as metadata, patch only the declared region, leave the file alone).
  Rejected: it *preserves* a foreign file by rewriting its header, so a file this project never
  written still gets a mutation it did not ask for, on a guess about which chunks are audio. The
  refusal is the conservative answer the hazard calls for, and it needs no assumption to be wrong.
- **Relieve `is_finalized` to `data_offset + data_bytes <= file_len` and leave `recover` unguarded.**
  Rejected: `recover` is a **public** function with non-pool callers, so the guard has to live in it.
  The two changes are the same decision, not two.
- **Refuse a take whose data chunk is not the file's tail *and* whose declared size is not the
  placeholder**, i.e. require positive proof this crate wrote the file. Rejected as unfalsifiable in
  the direction that matters: after `finalize` patches a real size there is no marker left to
  distinguish our file from a foreign one, so the rule would refuse exactly the takes recovery exists
  for. The short-of-declaration signal is the evidence that is actually present.
- **Truncate a foreign file to its data chunk** so the file matches the strict definition. Rejected
  as the destructive guess: it silently deletes a DAW's `cue` points and `INFO` tags from a user's
  file. Refusing loses nothing; truncation loses the metadata permanently.

## Consequences

- **Behaviour change — inputs now refused that were previously "recovered":** a WAV whose file
  length exceeds `data_offset + data_bytes` no longer has its header rewritten, and `recover` returns
  `Err` for it instead of `Ok(frames)`. In practice that is (a) any third-party WAV with a trailing
  `LIST`/`INFO`/`fact`/`cue`/`id3 ` chunk, (b) any 24-bit file with an odd frame count, and (c) any
  file with junk appended after its data chunk. All three were **corrupted** by the old path, so no
  user loses a working file — but a caller that was relying on the old `Ok` for such a file now sees
  an error naming the trailing bytes. `Pool::recover` surfaces that in `Recovery.errors` per source,
  non-fatally, and derives the source's peaks from the file as it stands.
- Recovery of a genuinely crashed take is **unchanged**: `crash_recovery_recovers_the_take`,
  `float_take_crash_recovers`, `crashed_take_is_recovered` (spike_b), `recover_finalizes_a_crashed_take`,
  `a_twenty_four_bit_take_recovers_to_its_full_length` and the new
  `a_crashed_take_that_is_short_of_its_own_declaration_recovers` all recover the same frames and cut
  the same torn tail. The placeholder path is untouched: `min(0xFFFF_FFFF, file_len) == file_len`.
- The media-pool note's "never mutates a well-formed source" invariant now **holds for foreign WAVs**,
  not only for files this crate wrote.
- Three regression tests, all of which fail on the pre-fix logic (verified by reverting the two
  predicates and re-running): `wav::tests::a_trailing_chunk_is_neither_crashed_nor_audio` and
  `wav::float_tests::a_riff_pad_byte_after_odd_data_is_neither_crashed_nor_audio` (each asserts
  `is_finalized` is true, `recover` refuses, and every byte is unchanged), and
  `pool::a_foreign_source_with_a_trailing_chunk_is_left_alone` (the pool-level shape: peaks still
  derived, source byte-identical, 1000 frames in the index).
- **Not this note:** `parse_header`'s chunk walk still skips a chunk's body without its RIFF
  word-alignment pad byte, so a file with an *odd-length chunk before* `data` still fails to parse
  (`wav.rs:142`'s else arm). That is a read-path defect, separate from this recovery-path decision.
- Tests: `cargo test -p media` green (154 tests, 7 pre-existing hardware/soak ignores).

*Authored with Space Bunny · OpenCode, 2026-09-29.*
