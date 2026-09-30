# Agent Note: A skipped chunk body carries its RIFF pad byte

Status: implemented

## Problem

`parse_header` walks the RIFF chunk list: eight header bytes, then either read the body (`fmt `), take
the data chunk's offset and stop (`data`), or **seek past the body** — every other chunk, which is a
`LIST`/`INFO`/`cue `/`bext`/`id3 `/`fact` to a reader that wants audio (`wav.rs`). The skip moved by
the declared size alone:

```rust
} else {
    reader
        .seek(SeekFrom::Current(size as i64))
        .map_err(|e| e.to_string())?;
}
```

A RIFF chunk body of odd length is followed by **one pad byte** so the next chunk starts on a word
boundary. So after an odd-sized unknown chunk the reader sits *on the pad*, and reads `[pad, 'd','a']`
as the next tag — which is not `b"data"`. It takes the data chunk's own size field as another unknown
chunk's size, seeks a few hundred kilobytes forward, hits EOF, and falls out of the loop to
`Err("missing data chunk")`.

The size was read correctly; only the arithmetic that consumes it was missing a byte. The
failure is a **valid foreign file refused**: `WavReader::open` errors, `Pool::list` files the source
under `errors`, `Pool::import` fails, and — because `WavWriter::is_finalized` and
`WavWriter::recover` go through the same walk — the pool's crash pass cannot even classify it. The
diagnostic says nothing about alignment, so the user is told the file has no data chunk rather than
that the reader lost its place.

Nothing in CI could see it: `write_header` emits exactly `RIFF`, `fmt `, `data` — no other chunk ever
exists in a file this crate writes, and the odd-sized-chunk fixtures the other tests hand-write
(`write_pcm24_padded`, `a_trailing_chunk_is_neither_crashed_nor_audio`) put their metadata *after*
`data`, where the loop has already stopped reading. Reported by the [Space Bunny review
2](../../../../research/architecture/2026-09-29-space-bunny-review-2-media-io.md) as Minor #6, carried
over unfixed from the [2026-08-24 full-codebase
review](../../../../research/architecture/2026-08-24-full-codebase-review.md) ("`wav.rs` chunk
skipping ignores word-alignment (`size & 1`)"), and confirmed independently. It is also the read-path
defect the [sample-width note](2026-09-29-recovery-uses-the-headers-sample-width.md) and the [recovery
salvage note](2026-09-29-recovery-salvages-only-what-is-there.md) each parked as not their own — this
is that note.

## Decision

**A chunk body is skipped with its pad byte: `size + (size & 1)`, in both arms that skip.**

- The unknown-chunk arm seeks `size as i64 + (size & 1) as i64`. The pad term is a local named `pad`,
  with a comment that says why it is there — the same shape the [recovery
  note](2026-09-29-recovery-salvages-only-what-is-there.md) already reasons about on the other side
  of the data chunk, now on the walk that *reaches* the data chunk.
- The `fmt `-overshoot seek (`size > 40`, the space a WAVEFORMATEXTENSIBLE body or a longer
  extension leaves after the first 40 bytes) carries the same term. It is the same defect in the same
  function: a body skipped without the byte that follows it. `pad` is 0 for every conforming `fmt`
  size (16, 18, 40), so the term cannot change an accepted file's parse; it makes the walk uniformly
  word-aligned rather than aligned in three of four places.
- The **data** chunk needs no pad: the loop breaks on `data` and `WavReader::open` re-seeks to
  `data_offset`, so a pad byte after the audio is metadata the reader never looks at. That is the
  [recovery note](2026-09-29-recovery-salvages-only-what-is-there.md)'s decision about trailing
  bytes, unchanged.

## Alternatives considered

- **Track a running offset and compare it against `data_offset` instead of trusting the walk.**
  Rejected: it makes the parser's position a second source of truth about the file's layout, and the
  pad byte is not something to work around — it is a term in the size. One byte of arithmetic is the
  whole defect.
- **Read a fixed maximum of bytes for every unknown chunk and only then skip** (what a stricter
  reader does to keep the stream seekable). Rejected: `parse_header` takes `impl Read + Seek` and
  seeks precisely so a large `bext` or embedded artwork costs nothing; reading a body we are going to
  discard is work on a path that runs on `Pool::list`.
- **Skip the pad only when the *next* tag fails to match** (peek, retry, forgive). Rejected: it makes
  a misaligned walk *recoverable* rather than correct, and it would silently absorb a real
  desynchronisation — a genuinely truncated chunk would look like a chunk that merely needed a pad.
- **Refuse a file that carries any chunk we do not understand.** Rejected: it makes the reader
  strictly less capable than `Pool::import` already is (it copies a foreign file byte for byte), and
  it converts a fixable arithmetic error into a refusal. The existing `fmt `/`data` knowledge is
  enough; only the *distance* between chunks was wrong.
- **Bundle the `fmt `-overshoot term with a test of its own.** Rejected: an odd `fmt` size over 40
  bytes with a PCM-or-float format tag is a shape no conforming writer emits (WAVEFORMATEXTENSIBLE
  is 0xFFFE, which `parse_header` refuses by format tag anyway), so a test would pin tolerance for a
  file that cannot occur. The term stays because it is the same rule, not because it is reachable.

## Consequences

- A foreign WAV with an odd-sized chunk before `data` now imports: the reader reaches the data chunk,
  and `is_finalized`/`recover` reach the same conclusion through the same walk. This is the common
  shape — a `LIST`/`INFO` text tag of odd length, a `cue ` point table, an `id3 ` chunk.
- Regression test, `wav::tests::an_odd_sized_chunk_before_data_is_skipped_with_its_pad_byte`: a
  1000-frame 16-bit mono take with an 11-byte `LIST`/`INFO` chunk and its pad spliced in front of
  `data`, asserting the reader opens, reports 48 kHz, and returns all 1000 frames at their own value,
  and that `is_finalized` still reports the file as a well-formed take. On the pre-fix logic it fails
  on the open, with `missing data chunk`.
- No other test moves: every file this crate writes, and every fixture in `wav.rs`, has its chunks at
  even offsets *before* `data`, so both pad terms are 0 on them (the one odd length among the
  fixtures is a pad *after* `data`, which the walk never reads).
  `a_trailing_chunk_is_neither_crashed_nor_audio` and
  `a_riff_pad_byte_after_odd_data_is_neither_crashed_nor_audio` are unchanged and still pass — the pad
  after `data` is still not audio and still survives recovery's refusal.
- The [Spike B note](../architecture/2026-08-17-spike-b-media-engine.md)'s `hound`-as-Phase-1-upgrade
  rationale is unchanged: format *edge cases* are still `hound`'s job. This was not an edge case, it
  was the format's word alignment.
- The review's other read-path items stay open and are not this note: `finalize` leaving the writer
  at byte 44 (Minor #7) and `Pool::import`'s verbatim copy of a foreign file it has not validated
  (Minor #6's `import` framing).
- Tests: `cargo test -p media --lib` green (126 passed, 3 pre-existing hardware/soak ignores).

*Authored with Space Bunny · OpenCode, 2026-09-29.*
