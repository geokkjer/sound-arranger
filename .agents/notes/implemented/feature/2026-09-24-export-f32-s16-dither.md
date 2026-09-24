# Agent Note: `export` — the deliverable sibling of `bounce`

Status: implemented

## Problem

The owner's alpha ask ends with "**export final mix**". The plan's item 13 fixes the shape: a
*separate* command, not an overload of `bounce`. `bounce <frames> <path>` exists and is
load-bearing — hand-computed frame count, 16-bit WAV, the byte-identical test role — but it is a
*measurement tool*, not a deliverable:

- a bounce renders from wherever the playhead is, so "the mix" depends on the transport state;
- it asks the user for a frame count, which is the documented trap the export must not leak;
- 16-bit with no dither is the wrong default for a master (the truncation error is audible on fades);
- nothing checks the result, so a mix over full scale is written clipped and the user finds out later.

## Decision

`export <path> [f32|s16]` — an **action** (it writes a file), host-side, with the arrangement's own
length:

- **A rebuilt clone renders it — the live session is never touched.** `HostSession::rebuild(None)`
  builds a fresh session from the state history and the render runs there, so an export does not move
  the transport, disturb the graph, or finalize a take in progress (the gate measured the playhead
  jumping to the arrangement's end and a recording being closed by a file-write gesture). It also
  makes an export a function of the *document*, not of where the playhead happened to be.
- **The state is applied at the present instant** (`HostCommand::at_now`: order decides, the frame
  placement is dropped) and the render starts at frame 0. So the export is the session's **current**
  value rendered from the start: a limiter mounted at `@5000` masters the whole file instead of being
  silently absent from the deliverable — which the gate showed as an export clipping while the audible
  session did not. A `seek` keeps its frame-gated replay; the two questions ("what is the session
  now" vs "what was it at frame N") are different paths, not one compromise.
- **The length is measured, never handed in**: `media::Timeline::end_frame()` (the last frame any
  clip occupies). An empty arrangement is refused by name (`nothing to export: the arrangement has no
  clips`) rather than writing an empty file.
- **f32 by default** (`WavWriter::create_float`): the mix exactly as rendered, bit-exact and
  golden-file testable. **`s16` on request**, through a **fixed-seed TPDF dither**
  (`media::TpdfDither`, `DITHER_SEED`).
- **Reports peak and RMS** — in the CLI summary, on the outcome/snapshot (`ExportStatus`, so a shell
  reads what was written instead of re-rendering), and in the TUI's status line.
- **Refuses rather than writing a clipped file**: a peak above full scale (`1.0`) is an error that
  says the peak in dBFS, names the likely fixes (the mix, or a master chain's ceiling), and writes
  *nothing*.
- **Recorded, not replayed**: an `ExportRecord { frames, format, peak, rms, drained_frames }` goes
  into the log as `MediaExport` (like `MediaBounce`), so a replay reproduces the log byte-for-byte
  while never re-rendering. The output **path is not recorded** (an action target, not session
  state — two identical sessions exporting to different files must log identically).

**The dither** (`crates/media/src/dither.rs`) is its own module because it is a *format* concern the
sound-sculptor profile will want too:

- **Triangular PDF**: the sum of two independent uniforms in `[0, 1)`, i.e. `r1 - r2` in `(-1, 1)`
  LSB. Triangular dither removes the first two moments of the quantisation error (its mean and its
  correlation with the signal), which is what turns truncation distortion into a steady, signal-
  independent noise floor. Tested as such: mean ≈ 0, variance ≈ 1/6 (rectangular would be 1/12, no
  dither a spike at 0), and a sub-LSB constant quantises to a *distribution* whose mean is the
  constant (the unbiasedness dither exists for).
- **Fixed seed, process-local**: `SplitMix64` from `DITHER_SEED`, never thread-local or time-based
  entropy. An export is therefore byte-reproducible (tested: two exports of the same session are
  identical, f32 and s16 alike), which is what the log-is-the-document rule requires of anything the
  product writes.
- **Exactly on the grid**: `quantize_s16` writes `q / 32767` for an integer `q`, so the 16-bit writer's
  `(s * 32767).round()` reproduces `q` with no double rounding. Non-finite input becomes silence.

**The TUI gesture** is `X` (`Action::ExportMix`, shared keymap — the iced shell reports the gap like
any unwired action): it opens the command line **prefilled** with `export <dir>/mix.wav f32` (beside
the script the session was opened from), so what will be written and in which format is on screen
before anything is. Enter runs it as it stands; typing `s16` first asks for the dithered file. The
"unfinished prefill" guard in `run_prompt` now keys on a **trailing space** (`R`'s
`arrange rename_track t0 ` still gets its hint; a complete export line runs).

## Alternatives considered

- **Extend `bounce` with a format argument** (one command, two behaviours). Rejected by the plan and
  by the tests: `bounce` is the measurement primitive the byte-identical replay tests lean on (fixed
  16-bit, explicit frames, from the current position), and an export has the opposite defaults in
  every one of those dimensions. Two commands with one job each beat one command with a mode flag.
- **f32 only.** Rejected: 16-bit is what most consumers and players want, and shipping it without
  dither would make the *default* deliverable worse than the tools it replaces.
- **Rectangular (RPDF) dither**, `r - 0.5`. Rejected: it removes the error's mean but leaves its
  *correlation with the signal* (the modulation noise), which is exactly what is audible on fades.
- **Noise shaping** (error feedback into the ultrasonic range). Kept open: it needs a filter and a
  coeffect (where the previous error lives); TPDF is the honest first step, and the module is the
  place a shaper would go.
- **A random seed per export** (or thread-local RNG). Rejected: it makes the same session export to
  different bytes, which breaks the reproducibility every other part of this project is built on.
  "Random" is not the requirement — *decorrelated* is, and a fixed seed gives that too.
- **Refuse to export when the peak exceeds 1.0 *or* resize the mix automatically** (a built-in
  limiter). Rejected: silently changing the mix is worse than refusing it; the master chain's
  limiter is the tool for that, and the error names it.
- **Refuse a peaking mix for f32 too.** Kept in: a float file *can* hold 1.4, but every 16-bit player
  and most converters clip it, so "the export is usable" is the standard, not "the file is
  representable". The peak is reported either way, so the user knows what to fix.
- **A `bounce`-style explicit frame count in `export`** (for a partial render). Rejected for the
  deliverable: that is what `bounce` is for. A range/selection export is a later feature with its own
  command.
- **Writing the export *into* the pool** (so it becomes material). Rejected: an export is a
  deliverable, not a source; a pool entry would make the master of a session part of its own pool and
  invite feeding it back in.

## Consequences

- **The alpha's last step is a key**: `X`, edit the path if wanted, Enter. The status says what was
  written and how hot it is; nothing is written if the mix would clip.
- **Exports are reproducible**: the same session exports to byte-identical f32 and s16 files, from
  any playhead position, on any machine — the dither's seed is a constant, not entropy.
- **`export` is not a second `bounce`**: the bounce primitives (hand-computed frames, current
  position, 16-bit, the replay tests) are untouched, and `bounce`'s record and `export`'s record are
  separate log ops with separate fields.
- **A mastering chain is now testable end to end**: export refuses a clipping mix, and with a
  `master` mounted the ceiling is what keeps it exportable — the two halves of item 13 meet in the
  refusal message.
- **A refusal never destroys anything**: an existing file at the target path is left untouched (a
  failed export must not delete what an earlier one wrote), and the error says "the file was not
  written" rather than "nothing was written" for exactly that reason.
- **The peak scan counts non-finite samples** rather than folding them away: `f32::max` ignores a
  NaN, so a naive peak reports 0 for a mix of NaNs and the f32 path would write them. A non-finite
  sample anywhere in the render is refused by count.
- **The 16-bit rails reserve a code for the dither.** A full-scale `1.0` scaled to 32767 and, with a
  TPDF offset up to +1 LSB, was clamped on a large fraction of samples — signal-correlated error
  exactly at full scale, the thing dither exists to remove. The sample is scaled to `±32766` *before*
  the offset, so `(-1, 1)` never reaches a rail and nothing is clamped; 32767 stays reachable through
  the dither itself.
- **The in-memory render bound is ~46 minutes of stereo at 48 kHz** (1 GiB, one buffer rather than a
  stream), stated in the refusal message with the command's own name. Stems (which would stream) are
  the way past it.
- **Still open**: stems (one file per track) and compressed formats (FLAC/MP3), a range/selection
  export, loudness (LUFS) and true-peak reporting rather than sample peak, and noise shaping (the
  dither module is where it would live). The iced shell now dispatches the shared gesture to its own
  prefilled prompt, so both shells have it. The export *report* is control-side convenience: a rebuild
  carries the last one across, and the engine log of the moment carries the `MediaExport` entry.

## The gate's findings

The slice's gate returned `merge with changes` ([review +
disposition](../../../../research/architecture/2026-09-24-alpha-slice-gate-f2-glm-standin.md)); both
must-fix and all five should-fix findings were real and are fixed, with tests:

1. **A NaN passed the clipping refusal** (must-fix) — the peak fold used `f32::max`, which *ignores*
   NaN, so the `is_finite` guard was dead code for NaN while the f32 writer stores samples unclamped.
   Found independently in self-review and already fixed when the gate reported; the measurement now
   counts non-finite samples and refuses by count.
2. **The export relocated the transport and finalized a recording** (must-fix) — it rendered through
   the live session. Fixed by rendering a rebuilt clone.
3. **State stamped after frame 0 was absent from the export** (should-fix) — a mid-session limiter
   mastered the audible mix but not the deliverable. Fixed by applying all state `at_now` in the
   export's rebuild.
4. **A stale file survived a refusal** (should-fix) — fixed in the message and by test: a refusal
   never touches an existing file.
5. **The dither hard-clamped at exactly full scale** (should-fix) — a rail code is now reserved before
   the offset.
6. **The budget refusal said "bounce"** (should-fix) — it now names the command and the memory bound.
7. **The TUI's report dedupe swallowed a repeat export** (should-fix) — the key is the whole report,
   and the status reports the file's length (`frames + drained`).

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-24.
