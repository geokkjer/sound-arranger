# Reviewer gate (stand-in) — alpha slice F2 (`export`), GLM-5.3

> Research input, 2026-09-24. Co-worker: **GLM-5.3** (`zai/glm-5.3`), read-only, standing in for the
> designated gate **Kimi K3** (unusable: every API run died with no output — see
> [`2026-09-23-kimi-gate-alpha-slices.md`](2026-09-23-kimi-gate-alpha-slices.md)).
> Scope: `export` as `bounce`'s deliverable sibling — the fixed-seed TPDF dither, the whole-arrangement
> render, the clipping refusal, the shell-visible report, and the TUI/iced gesture. Every finding was
> backed by an out-of-repo probe against the built rlibs or a `host v1` script (`/tmp/exp-probe/`).
>
> **Disposition: `merge with changes`; both must-fix and all five should-fix findings are fixed, with
> tests.** Two of them were real design errors rather than slips (the export rendering *through* the
> live session, and state stamped after frame 0 being silently absent), and both were fixed by
> changing what the command *is*: an export now renders a **rebuilt clone** whose state is applied
> **at the present instant**.

## Must-fix

1. **A NaN passed the clipping refusal.** The peak was `fold(0.0f32, |m, x| m.max(x.abs()))`, and
   `f32::max` *ignores* NaN, so `!peak.is_finite()` was dead code for NaN (it still caught `±inf`) —
   while the f32 writer stores samples unclamped, so a NaN reached the deliverable. The reviewer's
   probe: `fold` over `[0.5, NaN, 0.7]` → `0.7`; and the s16 path *silences* NaN, so the two formats
   disagreed about the same mix. **Found independently in self-review and already fixed when the gate
   reported** (the gate's probe confirms the finding): the measurement now scans for non-finite
   samples, counts them, and refuses by count. `export_writes_the_whole_arrangement_and_refuses_to_clip`
   covers the refusal paths.
2. **The export relocated the transport and finalized a take in progress.** `export()` called
   `replay_to(0)` and then rendered the whole arrangement *through the live session*, so the playhead
   ended at the arrangement's end (the gate measured 7000 → 9600) and a recording was finalized as a
   side effect of a file-write gesture. **Fixed structurally**: `export` now renders a **rebuilt
   clone** (`HostSession::rebuild(None)`), leaving the live session's transport, graph, recording and
   journal untouched. `an_export_leaves_the_session_alone_and_includes_later_state` asserts the
   playhead does not move.

## Should-fix

3. **State stamped `@frame > 0` was silently absent from the export** (the gate's probe: identical
   sessions with a limiter mounted at `@0` vs `@5000` exported **different bytes**, the later one
   unlimited — so the deliverable could clip while the audible session did not, contradicting the
   refusal message's own advice to mount a chain). **Fixed by design**: the export rebuild applies
   every state command **at the present instant** (`HostCommand::at_now`, order decides) and renders
   from 0, so the export is the session's *current* value rendered from the start. Re-verified at the
   CLI with a limiter mounted at `@3000`: the exported peak is exactly the −6 dB ceiling
   (0.5012), i.e. the mid-session limiter masters the whole file. A seek keeps its frame-gated
   semantics — the two questions ("what is the session now" vs "what was it at frame N") are
   different, and are now two different code paths.
4. **Stale-file hazard on refusal.** The peak check runs before the writer is created, so a refused
   export writes nothing — but a *previous* successful export at the same path stayed, readable as if
   it were the new one. **Fixed in the message and by test**: "the file was not written" (it never
   touches an existing file — a failed export must not destroy what an earlier one wrote), and
   `an_export_leaves_the_session_alone_and_includes_later_state` asserts the bytes are byte-identical
   after a refusal.
5. **Full-scale hard clip of the dither.** `peak > 1.0` admitted exactly `1.0`, and then
   `round(32767 + d)` could reach 32768 and was clamped — the gate measured 17 536 of 20 000 samples
   clamped at a constant `1.0`, i.e. signal-correlated error exactly at full scale. **Fixed**: one code
   is reserved at each rail *before* the dither (`scaled.clamp(-32766, 32766)`), so `(-1, 1)` of
   offset never reaches a rail and nothing is clamped, with 32767 still reachable through the dither.
   `full_scale_dithers_without_clamping` pins it (both top codes appear, nothing clamps).
6. **The budget refusal said "bounce" for an export** and hid the reason. **Fixed**: the message names
   the command and the memory bound ("~1 GiB in-memory render budget", ~46 min of stereo at 48 kHz),
   and the doc comment states the bound rather than leaving it as a surprise.
7. **The TUI's report dedupe key was `(frames, format)`**, so a repeat export of the same shape after
   a rebuild (a different mix) was never announced; the key is now the whole report
   (`frames, format, peak bits, rms bits`) and it is cleared when the host has no report at all. The
   status also now reports the **file's** length (`frames + drained`), which the earlier text
   under-reported.

## Notes (recorded, not code gaps)

- The iced shell now dispatches the shared `ExportMix` action to its own command line prefilled with
  `export mix.wav f32` (it has the same prompt), so the gesture is shared rather than a gap.
- `ExportStatus` carries `drained_frames` so a shell can report the file length truthfully.
- The export *report* is control-side convenience: a `seek` rebuild carries the last report across
  (the rebuild copies it), so a shell's readout no longer vanishes; the engine log of the moment still
  carries the `MediaExport` entry either way.
- `end_frame` is clip-based, so a session whose pool has material but whose arrangement is empty is
  refused by name ("nothing to export: the arrangement has no clips"), and a muted or gain-zeroed
  last track still contributes its span (trailing silence is the arrangement's length — an
  arrangement-length semantic, documented in the note).
- A ~46-minute stereo export hits the in-memory bound; that is a real alpha limit, stated in the
  refusal rather than discovered.

## Checked and correct (executed, not assumed)

- **The 16-bit grid round-trips exactly**: an exhaustive probe over every `q ∈ [-32767, 32767]`
  through the writer's `(s.clamp(-1,1) * 32767).round()` had **0 mismatches**; `±1.0`, `±32767` and
  `0` all exact — no double rounding.
- **The dither's statistics hold**: `r1 - r2` from 24-bit uniforms, mean ≈ 0, variance ≈ 1/6 LSB²,
  unbiased on a sub-LSB constant; SplitMix64 has full period; one interleaved stream decorrelates L/R;
  a fixed documented seed gives byte-identical s16 exports (twice, from different playheads).
- **Refusal ordering**: the check precedes writer creation, so a refused export produces no partial
  file, and (now) leaves an existing one alone.
- **Log determinism**: `MediaExport` is an engine op whose handler only records (no re-render, no file
  I/O); `export` never enters the host history, so `save`/`load`, the journal and a replayed log are
  unaffected and do not diverge.
- **The trailing-space prefill guard**: `R`'s prefill still gets its hint; `X`'s complete line runs
  as-is (TUI tests cover both).
- All suites green on the reviewed tree: media (dither unit tests), host, workspace, tui-shell,
  iced-shell, doc-tests.
