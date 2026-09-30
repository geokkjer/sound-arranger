# Agent Note: A recovered take is described at the header's own sample width

Status: implemented

## Problem

`WavWriter::recover` rescans a take that died mid-write, patches the two placeholder size fields,
`set_len`s the file to a frame boundary, and returns the frame count it found. It inferred the
**bytes per sample from the bit depth** — `h.bits == 32` passed as a `float` flag into `data_bytes`,
which knew only 4 or 2:

```rust
let frames = actual_bytes / h.block_align();               // block_align = channels * (bits / 8) — correct
let data_bytes = data_bytes(frames, h.channels, h.bits == 32)?;  // 24-bit → 2 — wrong
f.set_len(h.data_offset + data_bytes)
```

`frames` was right (it divides by the *parsed* block alignment) and `data_bytes` was two thirds of
the file, so the two disagreed by construction. For a 24-bit mono take of N frames: `N * 1 * 2 = 2N`
kept of `3N`, and `recover` returned `N` — the **pre-truncation** count, which is the number
`Pool::recover` records in `report.finalized` (`pool.rs`). The header then declared the shortened
size, `is_finalized` returned `true`, and the loss was permanent: a third of a user's audio,
silently, with a report claiming all of it was recovered. The peaks were rebuilt over what survived.

24-bit PCM is not an exotic shape here. `parse_header` accepts it explicitly (it is what other tools
write, and the [Spike B note](../architecture/2026-08-17-spike-b-media-engine.md) records the reader
as 16/24-bit PCM + 32-bit float), and `Pool::import` copies a rate-matching mono file byte for byte,
so 24-bit sources legitimately sit in a pool. Found by the Space Bunny review (claim
`2-media-io#1`), confirmed independently.

**The trigger is narrower than "a 24-bit file in the pool".** `Pool::recover` only calls `recover`
for a source where `is_finalized` is false, and a pristine third-party 24-bit file *is* finalized. The
reachable shapes are a take whose data-size field was never patched (our own crash shape) or a file
whose declared size disagrees with its length. No test covered a 24-bit recover at all: the four call
sites were 16-bit, float, 16-bit and float.

## Decision

**The width is the header's own — `bits / 8`, the same term `block_align` already used.**

- `Header::bytes_per_sample()` returns `self.bits / 8`, and `block_align` is now
  `channels * bytes_per_sample()`. `frames` and `data_bytes` are therefore derived from **one** fact
  about the file, so recovery's arithmetic and the file's own geometry cannot drift apart.
- `data_bytes` takes a **width** — `fn data_bytes(frames: u64, channels: u16, bytes_per_sample: u16)`
  — instead of a `float` flag. `patch_sizes` (the writer's own `finalize`) supplies `4`/`2` from the
  writer's format flag, because the writer only ever emits those two; `recover` supplies the parsed
  width. The helper keeps its single >4 GiB guard (the
  [hardening note](../feature/2026-08-24-hardening-wave-guard-park-rate-render.md) owns that
  decision) instead of growing a second entry point.
- **Nothing `parse_header` accepts is now undescribable**, so `recover` needs no new refusal to
  "reject what it cannot describe exactly": it constrains the format-tag/depth pairs itself (16- and
  24-bit PCM, 32-bit float, with a 32-bit *PCM* tag refused rather than guessed at), and `bits / 8` is
  exact for every header that survives that.
- Recovery on a well-formed 24-bit take is therefore a **no-op that agrees with itself**: the patched
  sizes are the sizes already declared, the length is unchanged, and the returned count is the count a
  reader will report.
- Regression test, `wav::float_tests::a_twenty_four_bit_take_recovers_to_its_full_length`: a 24-bit
  mono file with the placeholder data size `write_header` writes before any audio (so `is_finalized`
  is false and the pool's crash pass would call `recover`), asserting the returned frame count, that
  **every audio byte survived**, that the patched size is 3 bytes a sample, and that the reader then
  reads back all six frames at their own values. On the pre-fix logic it fails on the audio bytes:
  18 bytes became 12.

## Alternatives considered

- **Return `format` in `Header` and branch on `FMT_FLOAT`** (the review's suggested shape). Rejected:
  `bits / 8` is the same number without a second field that can disagree with `bits`, and the guard
  that makes 32-bit float the only 32-bit format already lives in `parse_header` — so the tag adds a
  place to be wrong rather than information.
- **Refuse 24-bit in `recover`** (recover only the writer's own formats). Rejected: recovery would
  become a permanent per-pass error for material the reader supports and the pool legitimately holds,
  where today it is a lossy no-op and after this fix it is a lossless one.
- **Keep `data_bytes(frames, channels, float)` and special-case 24-bit at the call site.** Rejected:
  the >4 GiB guard would either be duplicated or the helper would grow a second entry point. A width
  is the parameter the guard was written for.
- **Fix the trailing-chunk trigger in the same change** — see Consequences. Deliberately not bundled:
  it is a different defect with a different decision attached.

## Consequences

- 24-bit recovery is lossless, and `recover`'s count and the file's contents agree for every format
  `parse_header` accepts. 16-bit and 32-bit-float recovery are **byte-identical to before** — 2 and 4
  were already correct — so `crash_recovery_recovers_the_take`, `float_take_crash_recovers`,
  `crashed_take_is_recovered` and the two `Pool::recover` integration tests pass unchanged.
- `data_bytes`' signature changed. It is a private helper with three call sites (`patch_sizes`,
  `recover`, and its boundary test), so nothing outside `wav.rs` moved.
- **Superseded, the hazard this note left open:** `is_finalized` required
  `data_offset + declared == file_len`, so a third-party WAV carrying a trailing chunk
  (`LIST`/`INFO`/`fact`/`cue`) — or the RIFF pad byte after odd-length 24-bit data — read as a
  crashed take, and `recover`'s rescan counted those trailing bytes as audio and *inflated* the take
  rather than truncating it. Fixed by the [recovery salvage
  note](2026-09-29-recovery-salvages-only-what-is-there.md): `is_finalized` is now
  `data_offset + declared <= file_len` and `recover` refuses a file whose data chunk is not the
  file's tail. The media-pool note's "never mutates a well-formed source" invariant now holds for
  foreign WAVs too.
- **Still open, and not this note:** `parse_header`'s chunk walk seeks a chunk's body without the RIFF
  word-alignment pad byte, so a file with an odd-length chunk *before* `data` still fails to parse
  ("missing data chunk"). That is a read-path defect, not a recovery one.
- Tests: `cargo test -p media` green (147 tests, 6 pre-existing hardware/soak ignores).

*Authored with Space Bunny · OpenCode, 2026-09-29.*
