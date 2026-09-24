# Reviewer gate (stand-in) — alpha slice E2b (stretch materialisation / tempo match), GLM-5.3

> Research input, 2026-09-24. Co-worker: **GLM-5.3** (`zai/glm-5.3`), read-only,
> standing in for the designated gate **Kimi K3** (unusable: every API run died with no output — see
> [`2026-09-23-kimi-gate-alpha-slices.md`](2026-09-23-kimi-gate-alpha-slices.md)).
> Scope: the time-stretch slice's second half — `HostSession::stretch`, `HostCommand::Stretch` +
> `SetSourceTempo`, `media::ArrangeOp::Stretch`, `Pool::write_source`, the `W` gesture and
> `tempo_ratio`. The reviewer compiled and ran its own probes (in `/tmp/e2b`, none tracked) rather
> than reasoning from the diff, which is why every finding below carries executed evidence.
>
> **Disposition: `merge with changes`; the must-fix and all three should-fix findings were real and
> are fixed, with tests.** Verdict on the current tree: one must-fix (a process abort), three
> should-fix (cross-platform rename, an id the log cannot name back, two failure paths that leave a
> half-useful source), and a long "checked and correct" list the reviewer verified by execution.

## Must-fix

1. **A large `num` aborts the whole host process.** `HostSession::stretch` sized the output with
   `Vec::with_capacity(region.len() * num as usize / den as usize + 4_096)`. Executed:
   `stretch t0 c0 4294967295 1` on a 48 000-frame clip → `memory allocation of 824633720656384
   bytes failed`, SIGABRT, exit 134 — reachable from one **logged script line**, in a host whose
   other magnitudes are capped (1000 source tempos, `MIXER_CHANNELS_MAX`). Fixed: a published ratio
   bound (`MAX_STRETCH_RATIO`, 1000:1 — a tempo match lives well inside it) and a published output
   bound (`MAX_STRETCH_FRAMES`, two hours at 48 kHz), both refused with a message that names the
   limit, plus `try_reserve_exact` so a future bound mistake surfaces as an error rather than an
   abort. The same class of bug on the *input* side is fixed too: the region read is clamped to what
   the file holds (`src_len` is the clip's window, not a fact about the file) and reserved with
   `try_reserve_exact`. Verified after the fix: the gate's exact command now exits 1 with
   `stretch ratio 4294967295/1 is beyond the host's limit of 1000:1`.
   Regression tests: `stretching_a_clip_materialises_a_pool_source` (both operands refused, at
   `u32::MAX`) and `a_declared_region_longer_than_the_source_is_read_to_the_end` (a clip declaring
   ten billion frames reads 48 000 and renders `s1.stretch.0_48000.3_2`).

## Should-fix

2. **`write_source`'s overwrite is POSIX-only.** `fs::rename` over an existing destination fails on
   Windows, and the design *deliberately* re-writes a deterministic id (stretch → undo → stretch).
   Fixed defensively: if the rename fails, remove the destination and retry, so the atomic path is
   kept where it exists and the gesture works where it does not. (Not executed on Windows — no such
   box; the reviewer flagged it from `std` semantics and it is now a two-line fallback rather than a
   platform assumption.) Tested on Linux for the behaviour that matters: writing the same id twice
   replaces the source rather than duplicating it.
3. **`valid_id` did not match its own contract.** `write_source`'s doc promised a
   "whitespace-free" id while `valid_id` accepted `"a b"` — an id that can exist on disk but that no
   space-separated `host v1` line can ever name back. Fixed: whitespace is refused (the doc was
   right, the code was not), with tests for a space and a tab. The knock-on is fixed in the same
   spirit rather than left as a regression: `Pool::import` now **sanitizes** a filename stem
   (`My Take.wav` → `My_Take`) instead of failing the import, so a space in a filename still
   imports while the id stays addressable in the log.
4. **Two failure paths left a half-useful source with a misleading message.** A peaks-rebuild
   failure returned a bare error *after* the wav was renamed into place (the source is usable but
   unpeaked, and the panel marks it), and a failed `Arrange` apply after a successful render leaves
   an orphan file. Fixed the first: the error now says the audio was written and only the peaks
   failed. The orphan is documented as intended (a render is working material; a failed apply leaves
   it in the pool rather than deleting material the user asked for).

## Checked and correct (executed, not assumed)

- **Determinism / replay**: two independent sessions stretching the same region at 3/2 produce
  byte-identical wav + peaks and an identical `session.txt`; loading the saved session reproduces the
  arrangement value from `source_tempo` + `arrange stretch` alone (no re-render), `undo` returns the
  original reference, `redo` returns the render, and `source_tempos` survive both. Re-confirmed here
  end to end: two runs of the same script bounce byte-identically (md5 equal) *and* the saved
  session's own replay bounces to the same bytes.
- **Classification**: `SetSourceTempo` is state (journaled, saved, replayed on seek); `Stretch` is an
  action and `format_command` returns `None` for it, so the single logged entry is the `Arrange` op.
- **f64 round trip**: `source_tempo s1 90.123456789012345` saves as `90.12345678901235` — the same
  f64. Non-finite/negative/NaN bpm refused.
- **Ratio direction**: `num/den = output/input` consistently across `Stretch::for_len`, the host id
  and the TUI message (host 3/2 grows 48 000 → 73 024; warp 90 → 120 bpm gives 3/4 and shortens).
- **The region-keyed id** prevents two clips of one source at different offsets from overwriting each
  other's material (the reviewer's *first* `git diff` caught the slice mid-fix — the id gained the
  region key between its diff and its first test run, which is exactly the bug that motivated the
  change; on the current tree the fixture test `a_stretch_id_keys_on_the_region_not_the_source`
  passes).
- **`tempo_ratio`**: exact reduced rational (90.5 → 181/240), `None` for NaN/±inf/≤0, saturating
  f64→u64 cast, post-reduction u32 guard.
- **Parse/format round trip** for `stretch`, `source_tempo` and `arrange stretch`; arities enforced;
  the formatter's output reparses to the same op. Fade caps and every refusal message present.
- Full suites on the reviewed tree: media 94, host 33, workflow, tui-shell 52, iced 6 — all passing.

## Notes (no action demanded)

- A rendered source has no tempo recorded, so warping an already-warped clip says "no tempo recorded"
  for the new id. Honest, and the fix it names works; propagating the session tempo onto the render
  would make a second `W` a no-op but would add a second history entry to one gesture.
- Pool ids have no length bound; a chain of stretches grows the name (~24 bytes each). Pre-existing
  for any pool id (a take id is user-supplied), and only reachable after several nested warps of the
  same clip.
