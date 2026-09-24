# Reviewer gate (stand-in) — alpha slice C (the snap grid), GLM-5.3

> Research input, 2026-09-24. Co-worker: **GLM-5.3** (`zai/glm-5.3`), read-only, standing in for the
> designated gate **Kimi K3** (unavailable: its 5-hour API window was exhausted at the time —
> retry after the reset with the 256 k-context model id, see
> [`2026-09-23-kimi-gate-alpha-slices.md`](2026-09-23-kimi-gate-alpha-slices.md)).
> Scope: slice C (grid snap in the language and the shell), working tree at the time —
> `crates/media/src/snap.rs`, `crates/workflow/src/lib.rs`, the `snap=<frames>` modifier in
> `crates/host/src/lib.rs`, the tempo map on `Snapshot` (`live.rs`), the TUI's grid/snapping/ruler,
> the iced shell's shared grid state, and the tests. The reviewer was told to attack the claims, cite
> file:line, separate verified from inferred, and end with a verdict.
>
> **Disposition: `merge with changes`; every finding is dispositioned, four of them fixed.**
>
> 1. **`quantize_frames` could overflow on a huge frame** (should-fix) — `frame + half` unchecked, so
>    `transport seek 18446744073709551615 snap=2` panicked under overflow checks (the debug profile
>    the tests run in). **Already fixed before the review landed** — a self-review pass found it and
>    made the add saturating (with `quantize_frames(u64::MAX, 2)` as the regression test); the
>    reviewer saw the pre-fix line. The finding stands as a record of the bug class.
> 2. **A snapped fade did not say so** (should-fix) — `fade_to_playhead` quantized the playhead but
>    omitted the `snapped_note` the split/trim/click paths carry, so a fade point that jumped was
>    unexplained. Fixed, with an assertion in `clip_gain_and_fades_are_keys` (arming the bar grid and
>    placing the playhead off it).
> 3. **`set_tempo` accepts bpm 0/NaN and would collapse the grid to frame 0** (note) — **refuted**:
>    the *parser* accepts any `f64`, but execution runs the engine's own validation first
>    (`tempo must be finite and positive, got 0` / `got NaN`), and a refused command changes nothing.
>    Verified by running both scripts through the CLI.
> 4. **`,`/`.` used the spike's `SAMPLE_RATE` const, not the session rate, and seeks were not clamped**
>    (note) — fixed: the second-seek uses `sample_rate()` (a 44.1 kHz session moves 44 100 frames per
>    second), and every seek target is clamped to the arrangement through one `clamp_frame` helper
>    (`nudge`, `seek_grid`, `timeline_seek`).
> 5. **The ruler's 4096-step cap blanked most of a zoomed-out fine grid** (note) — a 30-minute view on
>    the 1/4 grid needs ~14 000 steps for 100 columns, so the far end of the row was blank. Fixed by
>    drawing **by cell** instead of by grid line (`ruler_marks`): each column asks whether a bar (or,
>    failing that, a grid line) falls in it, which is O(width) at every zoom and loses no bar number.
>    Regression test `the_musical_ruler_fills_the_row_at_any_zoom` (1.5 s and 30 minutes). The
>    meter-change *phase* half of the finding stays open and is documented in the code and the note:
>    continuing a bar count across a segment boundary needs a bar-phase notion the tempo map does not
>    have yet.
> 6. **`publish` cloned the tempo map twice** (note) — **already fixed before the review landed**
>    (one `session.tempo_map()` per publish, with the sample rate taken from the same clone so they
>    cannot disagree).
> 7. **Tie-rounding documentation** (note) — clarified in `quantize_frames`' doc comment (even steps
>    round the midpoint up; an odd step rounds the integer half, so `step = 5` maps 2 → 0 and 3 → 5).
>
> Reviewer-verified (recorded because they are the slice's load-bearing claims): the beat-domain math
> round-trips exactly over a multi-segment map (a 2M-frame probe) and a frame just before a tempo
> change snaps with the pre-change segment; snapping never moves a frame more than half a step + 1;
> the parser's modifier works on all five frame-operand commands in either order with `@frame`, and
> its four refusals hold, with the live and script paths sharing one implementation; nothing about the
> grid reaches the host or the log (the formatter writes the snapped frame, the save/journal are
> rebuilt from formatted history); the workflow cycle and keys are shared, and iced holds the same
> state while reporting the gap; the TUI's grid is default-off in both constructors and untouched by
> undo/redo/load; `[`/`]` semantics are self-consistent with `nearest`.

---

## Verbatim review

Adversarial review of grid-snap slice C. Verdict: **merge with changes**. All test suites pass (media/host/workflow, tui 34, iced 6, pty-check ok). No determinism/log leakage found. Two should-fix + several notes.

## Findings

1. **should-fix — `quantize_frames` overflows `u64` on a huge frame** — `crates/media/src/snap.rs:182` `((frame + half) / step) * step` with `frame + half` unchecked. Reachable from the public parser: `transport seek 18446744073709551615 snap=2` parses the u64 fine, then quantize adds `u64::MAX + 1`. Demonstrated (standalone replica of the function): panics under overflow checks (i.e. in the repo's own `cargo test`/debug profile), silently wraps to a wrong frame in release. This contradicts the parser's stated invariant ("the wire schema's public parser must not panic", host lib.rs ~2687). Fix: `frame / step + (frame % step) * 2 >= step` style or `checked_add` + saturate.

2. **should-fix — the fade edit snaps but its status never says so** — claim 5/note say "an edit the grid moved says so in its status". Implemented only for split (main.rs:1006), trim (main.rs:1093), click-seek (main.rs:1450). `set_clip_fade` (main.rs:1231) uses `playhead = self.snap_frame(self.snap.frame)` and reports only the value — with the grid armed, a fade point that jumped to a grid line is unexplained, exactly the failure the note says it prevents.

3. **note — bpm 0/NaN is accepted by `set_tempo` and collapses the shell's grid to frame 0** — host lib.rs:2225-2234 parses any f64 (no validation); `TempoMap::frame_at` then falls back to segment start (verified by probe: `frame_at(1.0)` with bpm 0 → 0, NaN → start_frame; no panic). `media`'s constant-tempo helpers guard bpm (snap.rs:145), but the TUI's `snap_frame` goes through the map, so every snapped edit pins to frame 0. Degrade-not-crash holds; "usable value" is generous.

4. **note — `,`/`.` nudge still uses the `SAMPLE_RATE` const, not the session rate** — main.rs:599 `seconds * SAMPLE_RATE as i64`, then quantizes against `snap.tempo_map` (session rate). Inconsistent with `seek_grid`'s off-path which uses `self.sample_rate()` (arrangement rate). Pre-existing rate bug, now visibly inconsistent between two seek gestures. Also `nudge`/`seek_grid` don't clamp to `arrangement.frames` while `timeline_seek` does (main.rs:1448) — a `]` can seek past the end (transport tolerates it).

5. **note — ruler's 4096-line cap blanks most of a zoomed-out fine grid** — timeline.rs `draw_musical_ruler`: `count.min(4096)` over grid steps from the viewport start. A 30-min view at 120 bpm with the 1/4 grid needs ~14 400 steps; only the first 1024 beats get ticks/labels, the rest of the row is blank — the claimed degradation ("beat ticks alone") doesn't cover this. Also meter-change-mid-view: ticks and bar numbers use `meter_at(view.start)` while `snap_frame` uses `meter_at(edit_frame)`, so the ruler can disagree with what an edit snaps to across a meter change (the note admits the numbering half of this).

6. **note — `publish` clones the tempo map twice per publish** — live.rs:491-492 calls `session.tempo_map()` twice (each a full `Vec` clone). Runs on the actor thread per tick/command, not the audio callback — cheap for a few segments, but one clone would do.

7. **note — tie/rounding doc vs behaviour** — `quantize_frames` with an odd step (5) maps 2→0 (integer `half=2`), so "half a step rounds up" is only exact for even steps; `Grid::nearest` rounds half *away from zero*, so a negative tie rounds down (unreachable post-clamp). Cosmetic.

## Verified correct
- Beat-domain math: `TempoMap` frame↔beat round-trips exactly over 2M frames with 3 segments (probe); snapping a frame just before a tempo change uses the pre-change segment (479 000 → beat 20 → frame 480 000, exactly the change frame); degenerate meter 0 clamps to 1 (probe); nearest never moves more than half a step + 1 frame at 120/48k, 97.3/44.1k, 60/44.1k.
- Parser: `snap=` on all five frame-operand commands, either order with `@frame`; `snap=0`/`snap=abc`/no-frame-operand/`transport stop` refused (test + read); duplicated `snap=` fails via `exact()`/operand parse; `group begin|end` with `snap=` fails the `snap_used` check (read, INFERRED not executed); mid-line `snap=` fails operand parse; live (`parse_arrange_line`) and script paths share `parse_arrange` — identical.
- Determinism: `format_command` emits the snapped frame, never `snap=` (test asserts `arrange move_clip t0 c0 48000 @0`); `script_text`/save/journal are built from formatted history — no grid state anywhere. Nothing about the grid reaches host or log.
- workflow cycle off→bar→beat→1/2→1/4→off, off by default, keys `b`, `[`/`]` (verified + tests). iced holds the same Grid, cycles, reports nothing snaps.
- TUI: grid default-off in both constructors; undo/redo/load don't touch it (deliberate UI state); `timeline_seek` clamps post-snap; `H`/`L` clamp at 0, quantize the target, use local segment tempo; `[`/`]` half-step-offset semantics are self-consistent with `nearest` (from a position 60% toward the next line, `]` skips to the line after next because the snapped "current" line is the nearer one — deliberate, matches nearest).
- Snapshot: `tempo_map`/`sample_rate` follow the session every publish (rate/tempo changes and loads converge on next publish); `Default` is the documented 48k/120 demo shape only until first publish.

## Not verified
- Real-device audio behaviour and 44.1 kHz session end-to-end (silent host only; engine clock rate vs device rate is pre-existing territory).
- The exact release-mode wrapped value of finding 1 (observed MAX−1 in one build, 0 expected in another — profile-dependent; the panic under overflow checks is confirmed).
- Bar-number label collision at 5-digit bar numbers in the ruler (reasoned safe: later `|` writes overwrite label chars, ticks only fill blanks).
