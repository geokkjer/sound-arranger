# kimi review — P1.3.3 media pool (slice 4, 2026-08-24)

Session: `session_87f5c065-7b07-40a7-8e8c-8aebf499836e`

kimi reviewed `Pool::list`/`Pool::recover` + the `wav`/`peaks` edges. Verdict:
deterministic and mostly sound, but a crash day is exactly when the pool dir is most
likely to hold a weird file — and the current code aborts the whole index on the first one,
defeating the slice's purpose.

## Must-fix (both integrated)

1. **One malformed / truncated `.wav` aborts the whole `list()`**, and `recover()` calls
   `list()?` first, so recovery never runs — the day after a crash is precisely when a
   broken file is present. **Resolved:** per-source errors are isolated — `list()` skips a
   bad file and reports it (`PoolIndex.errors`); `recover()` collects per-source failures
   into `Recovery.errors` instead of `?`-propagating.
2. **A corrupt-but-present `.peaks` is never detected or rebuilt.** `peaks_missing:
   !is_file()` is `false` once a truncated sidecar exists, so `recover()` skips it forever.
   **Resolved:** `PeakFile::write` is now atomic (write to `*.tmp` + `rename`, so "exists ⇒
   valid"); `recover()` validates with `PeakFile::read` (magic + frame cross-check) rather
   than `is_file()`.

## Should-fix (integrated)

- **`Pool::path_for` was a path-traversal hole** — a clip id containing `/` or `..` escaped
  the pool dir. Now validates the id (rejects a non-plain stem) before joining.
- **No `finalized` signal in `PoolSource`** — the UI couldn't distinguish a healthy take
  from a crashed-pending-recover one. Added `finalized: bool` (computed in `list`, reused by
  `recover`, so no triple header parse).
- **`PeakFile::read` trusted the length prefix (potential OOM)** — a hostile/corrupt sidecar
  could claim 4 billion bins. Bounded by `frames / base_bin` / remaining length before
  allocating.
- **Redundant peaks check** (`peaks_missing || !is_file`) — folded into the read-validated
  check.

## Worth-considering (left as flags)

- **`WavReader::read_into` swallows I/O errors** (turns a mid-stream error into silent EOF);
  the pool's rebuilt peaks could be short and disagree with the source. Changing its
  signature ripples to the streaming path (core), so it's a follow-up, not this slice. A
  `peak frames vs wav frames` cross-check in `recover` (via `PeakFile::read`) now catches it.
- **u32 data-size ceiling** — 4 GiB ≈ 3.1 h stereo at 48 kHz float, *inside* the long-jam
  use case; a header wrap makes `is_finalized` never agree. Should error loudly on
  `u32::MAX` overflow (RF64 later). Deferred.
- **Lexicographic sort** (`take-10` < `take-2`) is deterministic anyway; case-sensitive
  `.WAV` skip is fine for a writer-controlled pool.

The full critique text is the reviewer's response in this session.
