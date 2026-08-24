# Agent Note: review-driven edge hardening — devices per-frame I/O + parse_script robustness

Status: implemented

## Problem

A full-codebase review (GLM-5.3, `research/architecture/2026-08-24-full-codebase-review.md`)
verified 4 critical bugs at edges the test suite could not see. Two of them are fixed here:
the cpal device callbacks were **per-sample, not per-frame** (so a stereo output played
every other sample at half rate, and a stereo input recorded at 2× rate with channels
alternated — both invisible to CI because the hardware tests are `#[ignore]`d and only
assert the ring drained), and `parse_script` indexed `words[N]` directly (so a truncated
command line panicked with index-out-of-bounds, bypassing the designed `bad script`/exit-2
path of the wire schema that a future Tauri shell feeds).

## Decision

**`devices.rs`** — `fill_output` pops one sample **per frame** and duplicates it across the
frame's channels (`data.chunks_mut(channels)`, one pop per frame); `fill_input` pushes one
sample **per frame** (channel 0) from an interleaved input buffer. `channels.max(1)` guards
`chunks_mut(0)`. Both callbacks delegate to the (unit-tested, no-device) helpers, so the
per-frame contract is pinned without real hardware.

**`host/lib.rs` `parse_script`** — a `word(&words, idx, at)` helper returns a clean
`Err("missing command operand")` for any missing operand; every arm (`mount`, `patch`,
`set_param`, `set_tempo`, `unmount`, `play`, `splice`, `record`, `bounce`) routes through it.
The mount params slice (`&words[2..]`) is safe because the plugin operand is validated first.
`at` is now `header_line + lineno + 1` (tracking the actual 1-based version-header line), so
error messages stay correct across leading blank/comment lines.

## Alternatives considered

- **Fix only the output callback (the review's headline)** — rejected: the input path had the
  exact mirror bug on the same device-interface class, and the two are the same per-frame
  contract. Fixed both.
- **Input as "intentionally mono" (push every sample)** — rejected: a stereo input would
  record at 2× rate with channels alternated; taking channel 0 per frame is the mono contract.
- **Keep `at = lineno + 2`** — rejected: it breaks when leading blanks precede `host v1`;
  tracking `header_line` is the correct arithmetic.
- **Index-guard only the flagged arms** — rejected: the sweep is uniform; `patch`/`port_ref`
  already used `words.get`, and its cases were added to the robustness test so no arm is
  untested.

## Consequences

- Real stereo hardware works: each output frame is duplicated across channels (one sample
  per frame), and a stereo input records channel 0 per frame (not every sample).
- `parse_script` never panics on a truncated line — it returns the designed parse error, so
  a typo'd/truncated UI command (the Tauri shell's future input) is an `Err`, not a crash.
- 5 new devices tests (stereo duplication, mono passthrough, silence-when-empty,
  zero-channel degradation, stereo-input one-per-frame) + a `parse_script_robustness` suite
  (each truncated line refuses cleanly, malformed operands refuse, a valid script still
  parses). 18 workspace suites green, clippy clean.
- **Deferred** (from the same review): WAV >4 GiB guard, player-retire free, rate-mismatch
  refusal, `render()` → `Result`; and the minors (ring false-sharing, `pending` VecDeque
  alloc, public-API panics, mount-param validation).
