# kimi review — WAV guard, player-retire park, rate check, render Result (2026-08-24)

Session: `session_f9fffd6d-3a60-453b-8d36-1cf15a49802e`

kimi reviewed fixes #3–6. Verdict: #3 and #5 merge after a small boundary fix; #4 and #6 are
correct at the reported site but adjacent paths need conscious handling.

## #3 — WAV >4 GiB guard (merged with the pad fix)

- **Should-fix (applied): the pad byte.** The RIFF size field counts a pad byte for an odd
  `data_bytes`, so `> u32::MAX - 36` is off by one at the extreme. Changed the guard to
  `> u32::MAX - 37`, and the boundary test pins `frames*4` just under/over it.
- **Worth-considering (noted): `recover` now refuses a header claiming > u32 frames.** For a
  recovery path a clamp-and-recover is defensible, but refusing is fine here; RF64 is the
  real answer (deferred).

## #4 — player-retire park (merged; adjacent path documented)

- **Verified: `FilePlayer::drop` detaches (no join),** so the retire path does
  **not** block the render thread for a sleep quantum — the park is safe.
- **The park fixes the reported EOF case** (clip A finished before the splice): the reader
  keeps its Arc clones alive past EOF, so when the player drops on the render thread the ring
  frees on the reader's own exit, not the audio thread.
- **Adjacent path (documented follow-up): the mid-read retire race.** If a player drops while
  the reader is still reading (the common splice-during-playback path), the reader's early
  `stop` return can race the player's `ring` drop — usually the free lands on the reader
  thread, but the ordering is nondeterministic. A disposal queue drained off the render thread
  would close it uniformly (but allocates on the render path, a no-alloc tension — worth a
  dedicated look). Also: parked readers accumulate per finished-but-current clip in the
  arranger until node drop (resource shape, documented).

## #5 — rate-mismatch refusal (merged)

- Correct place (construction), good named error. `DriftCompensator` exists in the tree
  (`media::drift`), so the message's remedy is valid (not a phantom).
- The double-open (`WavReader` in `new` + `FilePlayer`'s own) is a negligible control-side
  header read + a TOCTOU window for pathological local-file replacement — accepted.

## #6 — render → Result + unmount cleanup (merged; sweep + edge noted)

- **Applied a host-wide panic sweep:** the only remaining `.expect` (editor after
  `ensure_editor`) is now a clean `Err`; the CLI stdin read now exits 2 (not panics) on a
  read error. `render()`'s `Result` propagation is compiler-enforced (no caller wants a bare
  `Vec`).
- **Edge (documented):** clearing `player_mailbox`/`*_underruns` on mixer unmount is a partial
  teardown — an in-flight player across an unmount→remount loses delivery. Rare; noted.
- **Worth-considering:** `Result<Vec<f32>, String>` allocates an error `String` per failure —
  fine for the script/test host; if `render` were ever called per-callback from real time, a
  non-string error would be needed.

The full critique is the reviewer's response in this session.
