# kimi review — edge hardening, devices per-frame + parse_script robustness (2026-08-24)

Session: `session_45baebf9-b1b4-46be-bd9b-747c0f6feaa7`

kimi reviewed the two review-driven fixes (#1 devices per-frame output/input,
#2 parse_script `word()` sweep) before commit. Verdict: both directionally correct;
FIX 1 needed the input-side twin and FIX 2 needed the `patch`/`port_ref` verification and
the line-numbering fix.

## Must-fix (verified + integrated)

- **Verify `port_ref` bounds-checks (`patch` arm).** It does — `port_ref` uses
  `words.get(idx).ok_or_else(...)` (the original GLM review praised it). Added truncated
  `patch` / `patch euclidean.triggers` cases so the one untested arm is now covered by the
  robustness test (they refuse cleanly, never panic).

## Should-fix (integrated)

- **Input-side twin of the output bug.** `build_input` pushed every interleaved sample
  into a mono ring (`[L0,R0,L1,R1,…]`), so a stereo input records at 2× rate with channels
  alternated. Now `fill_input` pushes one sample per **frame** (channel 0) — the mirror of
  `fill_output`. Unit test asserts one sample per frame on a stereo input.
- **`at` line numbering.** `at = lineno + 2` was wrong when leading blank/comment lines
  precede `host v1`. `parse_script` now tracks the actual 1-based `header_line` and
  `at = header_line + lineno + 1`, so error messages are correct with leading blanks.

## Worth-considering (integrated as tests/notes)

- **Zero-channel guard documented.** `fill_output`'s `channels.max(1)` avoids `chunks_mut(0)`
  panicking; a `channels == 0` test pins the mono-degradation behavior so a cleanup-minded
  reader doesn't delete the guard.
- **`word()` error wording** (`"(need N)"`) is slightly ambiguous — a per-operand name would be
  clearer, non-blocking.
- An Agent Note is required for both (non-trivial); the input-downmix decision is captured
  in it.

The full critique is the reviewer's response in this session.
