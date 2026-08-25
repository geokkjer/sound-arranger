# kimi review — text-format `pool`/`arrange` (2026-08-24)

Session: `session_db474063-e3ab-4c1c-bccc-40575eb8273d`

kimi reviewed the text-format parser for `pool`/`arrange` (GLM-5.3 major #7). Verdict:
the operand-mapping off-by-one in `add_clip` (caught by a test) was genuinely fixed, but
there were a width truncation bug, non-finite-float acceptance, and a diagnostics off-by-one.

## Must-fix (integrated)

- **`loop_region` `u(3)? as u32` silently truncated** — `u()` parses u64 then `as u32` wraps
  (`4294967296` → `times: 0`). Now `u32::try_from(u(3)?)` refuses on overflow.
- **`f()` accepted `NaN`/`inf`** — a NaN gain reaches the audio path and makes byte-identity
  platform-fragile. Now rejects `!v.is_finite()`.
- **Diagnostics off-by-one** — helpers reported `operand {i+1}` where `i` is the word index
  (words[0]=op), so `add_track`'s missing track said "operand 2". Now `operand {i}` (a script
  author counts operands after the op).

## Should-fix (integrated)

- **Strict arity** — `add_track t1 junk` parsed fine (trailing junk silently accepted). Each op
  now has an explicit word-count check (`arity(n)`) — `add_clip` allows 10 or 11 words (the
  `[loop_len]` optional is the one documented exception). Tests assert trailing junk, NaN gain,
  and the u32 overflow all refuse.

## Worth-considering / noted

- **Namespace**: the snake_case text tokens (`add_track`) are the public wire contract
  (authoritative, versioned); the `AddTrack` variants are internal Rust. New variants need a
  text arm — the parse test covers the 12 now, but an exhaustive-enumeration round-trip test is
  a follow-up.
- **`pool <dir>`**: whitespace paths are impossible (whitespace-split) and `../` traversal is
  unimpeded (fine for the reference host — a future trusted-UI parser would re-validate). The
  manifest is the pool's own.
- **`@frame` vs `[loop_len]` positional ambiguity**: a forgotten `@` on an `add_clip` could be
  read as a `loop_len`. Documented.
- **Byte-identity test**: compares two `run_script` runs of the same script (proves
  determinism, the replay guarantee); a golden-baseline-against-the-ClipEditor-API check is a
  follow-up.
- **`u64::from_str` accepts a leading `+`** — harmless, noted.

The full critique is the reviewer's response in this session.
