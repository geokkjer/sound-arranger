# Merge gate — PR #13, `slice/iced-timeline` (GLM-5.3, full)

> Research input, 2026-10-04. Gate: **GLM-5.3** (full, `zai` route) via the `pi` harness, read-only,
> against `slice/iced-timeline` at `94f455f`, base `main` = `ebf8fe6`. The author is
> DeepSeek-V4.1-Flash, so the cross-vendor independence rule holds. The gate re-ran the gates it
> could (`cargo fmt --all --check`, the notes verifier, clippy `-D warnings` on `host` and both
> spikes, workspace 422 passed, spike tests 19 passed) and read the code.
>
> **Verdict: `merge with changes`.** The cache lifecycle, the cost model, the error channel and the
> canvas geometry were all VERIFIED sound; the gate asked for two changes — the note's
> refresh-point list was imprecise, and the span memory survived a `:load` — and both are fixed in
> `7ae3c3d`. Two degenerate limitations and two pre-existing observations are recorded rather than
> fixed. The delta after the gate was the gate's own two asks plus one test; it was verified by the
> gates and CI rather than re-gated, and that is stated here rather than implied.

## Disposition

| Gate finding | Verdict | Disposition |
|---|---|---|
| §1 The cache lifecycle: every arrangement mutation reaches a refresh (applied state command; `carry_over`; `from_script`; `apply_journal`; `Request::Load`; construction), and the candidate traps — a non-`is_state()` command that edits (`Stretch` terminates in `execute(Arrange)`), a rebuild without `carry_over`, a refused rebuild, a journal after the script refresh — all REFUTED | VERIFIED | No change. |
| §1 A refused command can leave the cache one step behind the live editor (`ClipEditor::apply` mutates the value, then the log append can refuse) | INFERRED, theoretical, **pre-existing** | Recorded, not fixed: the failure needs `validate_op` to reject a canonically-encoded op the timeline already accepted, and the seam predates this slice. A cache that snapshots the value matching the history is the designed behaviour. |
| §2 Cost: `publish` clones an `Arc` plus an `Option<String>`; all three replay walks call `process` (no per-step refresh); the `Arc`'s pointee is replaced wholesale, never mutated in place, so a shell's clone cannot tear | VERIFIED | No change. |
| §3 The error channel: `refresh_timeline` sets exactly one of `timeline`/`error`; both-`None` is only the pre-first-publish window and the poisoned-lock fallback, and the canvas draws that as its own honest third state; no path reaches the lane bed with an error set | VERIFIED | No change. |
| §3 Cosmetic: the caption still prints `0 clip(s)` beside an error canvas | CONFIRMED, cosmetic | **Recorded for a follow-up** (the caption should say the arrangement is unavailable when `error` is set). |
| §4 Geometry: zero span, zero/negative width, sub-pixel clips, the playhead clamp and the gutter overlap are all handled; the mapping and span tests are discriminating | VERIFIED | No change. |
| §4 A very wide arrangement (≥26 tracks) overflows the canvas: `timeline_height` caps at 260 pt but `draw` floors `lane_height` at 4 pt without re-fitting, so the last lanes clip | CONFIRMED, degenerate, no panic and no wrong data | **Recorded for a follow-up.** The fix shape: drop the 4 pt floor (or grow the cap) so lanes shrink to fit. `AddTrack` is unbounded upstream, so this is reachable. |
| §5 The note's refresh-point list omitted `apply_journal`, claiming a load refreshes once when a journaled load refreshes twice | CONFIRMED | **Fixed in `7ae3c3d`**: the note names both refreshes and says the journal's is the answer. |
| §6 `span_frames` survived a `:load`, so a shorter loaded piece was drawn compressed into the left of a canvas whose span the previous session had set — the flow the timeline was named for | CONFIRMED | **Fixed in `7ae3c3d`**: `advance_span` resets the played-span memory when the transport moves backwards (a load at frame 0, a seek, a rewind), with a unit test for growth, reset and regrowth. |
| §6 `timeline_span` folds an `end_frame()` `Err` (a clip span that overflows) to 0, so the canvas leans on the played span and clamps | CONFIRMED, consistent with the documented clamp contract | Recorded for completeness. |

## The review, verbatim

# Merge gate review — PR #13, `slice/iced-timeline` (94f455f)

## 1. Cache lifecycle — VERIFIED sound, no stale-or-wrong path found

Every mutation of the arrangement value reaches a refresh:

| Path | Refresh | Evidence |
|---|---|---|
| Applied state command | `execute` post-`process`, guarded `result.is_ok() && cmd.is_state()` | `lib.rs:2903-2911` |
| Seek / undo / redo rebuild | `carry_over` → `refresh_timeline`, before `*self = rebuilt` (both `*self =` sites at `lib.rs:3133`, `3207`) | `lib.rs:3160, 3183-3186` |
| Script load | end of `from_script_with` | `lib.rs:2895-2897` |
| Journal on load | end of `apply_journal` | `lib.rs:1602-1603` |
| Actor `Request::Load` | builds via `from_script`, adopts whole | `live.rs:472-493` |
| Construction | `new_at_with` (all constructors funnel here) | `lib.rs:772-776` |

The traps I specifically hunted, all REFUTED as defects:

- **A non-`is_state()` command that changes the arrangement**: the only candidate is `Stretch` (an action, `lib.rs:280`), but it terminates in `self.execute(&HostCommand::Arrange{…})` (`lib.rs:1297-1305`), which refreshes. `RecordStop`'s `commit_state(Take)` (`lib.rs:1988-1996`) touches `last_take`/pool, not the editor. `Load` refreshes via `from_script`/`apply_journal` before `*self = loaded` (`lib.rs:1523`). `Save`'s dry run builds a throwaway session.
- **A rebuild without `carry_over`**: none — `replay_to_kind` and `replay_full` both call `carry_over` before adoption; a refused rebuild propagates `Err` *before* `carry_over` (`lib.rs:3085-3087`), leaving `self` untouched.
- **Journal after script refresh**: refreshes again at `apply_journal`'s end; correct final value, adopted only after.
- **Refused command half-applies**: `ClipEditor::apply` mutates the live value *then* calls `engine.arrange_logged` (`clip_editor.rs:554-566`); if the log append failed after the value mutation, `execute` returns `Err`, skips the refresh, and cache ≠ live value. INFERRED, theoretical only — the failure requires `validate_op` (`engine/render.rs:1255-1263`) to reject a canonically-encoded op the timeline already accepted, and the seam predates this slice (unchanged file). Not a blocker; the cache merely snapshots the value matching the history.

## 2. Cost — VERIFIED

`publish` runs per pump tick and copies `session.timeline_status().clone()` — an `Arc` clone plus an `Option<String>` (`live.rs:734-736`); the pump and `execute` share one thread in the actor loop, so no race on the field. All three replay loops call `process`, which never refreshes (`rebuild` `lib.rs:2984/2991`, `from_script_with` `lib.rs:2888`, `apply_journal` `lib.rs:1593`) — one refresh per walk, not per step. The old `Arc`'s pointee is replaced wholesale by `*tl = tl.apply(op)`, never mutated in place, so the shell's clone can't tear. `build_outcome` still deep-clones via `session.arrangement()` (`live.rs:644`), but the shell calls `outcome()` only at boot, per `:` command, and at record-stop — not per frame. No user-visible per-command log walk remains.

## 3. Error channel — VERIFIED

`refresh_timeline` sets exactly one of `timeline`/`error` (`lib.rs:873-884`). Both-`None` exists only via `Snapshot::default()`: the pre-first-publish window (`live.rs:305`) and the poisoned-lock fallback (`live.rs:373`) — and the canvas draws that as "waiting for the arrangement on the snapshot" (`main.rs:1251-1261`), a third, honest state. The error is drawn as itself with an early return on the only draw path (`main.rs:1243-1250`); no path reaches the lane bed with an error set. Cosmetic only: the caption still prints "0 clip(s)" beside an error canvas.

## 4. Canvas geometry — VERIFIED, one degenerate limitation

- Zero span, zero/negative width guarded (`frame_x`, `main.rs:1128-1134`); clip widths clamped `.max(0.0)`/`.max(1.0)`, sub-pixel clips skipped; playhead clamped inside the right padding; gutter (64 pt, label max-width 56 at x=4) cannot sit under clips; `sample_rate.max(1)` in the caption.
- **Limitation**: `timeline_height` caps at 260 pt (`main.rs:1160-1163`) but `draw`'s lane math floors `lane_height` at 4 pt without re-fitting (`main.rs:1227-1229`), so at ≥26 tracks (no upstream bound — `timeline.rs:522-532` `AddTrack` is unbounded) lanes run past the canvas bottom and are clipped invisible. Degenerate display, no panic, no wrong data — acceptable in a spike, worth a follow-up.
- Tests: `the_clip_mapping_is_bounded` and `the_span_is_the_arrangement_end_or_the_furthest_frame_played` are discriminating (they'd catch a lost clamp, `min` for `max`, error-treated-as-value). The canvas test pins construction, not `draw()` — but says so honestly ("there is no renderer here, so this is where 'builds' is pinned").

## 5. Note vs reality — one imprecision

The note's refresh-point enumeration ("the end of the public `execute` … `carry_over` … and once at the end of `from_script`") **omits `apply_journal`** (`lib.rs:1602`): a load with journal entries refreshes twice, not "once". Behavior is correct (conservative, never stale), and item 3's per-walk claim is accurate, but the exhaustive-sounding list understates shipped reality — this repo's standing order is that an implemented note states it exactly. Everything else checks out: all three replay loops do call `process`; the supersede pointer on the MVP note's item 5 is honest and in place; `position_fraction` and the placeholder are gone; `HostOutcome.arrangement` is untouched.

## 6. Missed by earlier gates and this slice

- **`span_frames` survives a `:load`** (`main.rs` — assigned only at init and `Tick`'s `max`). Play session A deep, `:load` a shorter session B: B's clips draw compressed into the left sliver, because the span is `max(end, stale played span)`. The note's "a loaded piece shows its whole length before it is played" holds only when the previous span ≤ the new end. Low severity (proportions and playhead stay correct), spike scope, but it is the one flow this feature was named for.
- `timeline_span` folds an `end_frame()` `Err` (clip-span overflow) to 0 (`main.rs:1117-1123`); the canvas then leans on the played span and clamps — consistent with the documented clamp contract, noted here for completeness.

Gates run: `cargo fmt --all --check` ✓, notes verifier ✓, clippy `-D warnings` on `host` and the spike ✓, workspace tests 422 passed / 0 failed, spike tests 19 passed.

---

**VERDICT: merge with changes** — the cache lifecycle, cost model, error channel and geometry are correct and well-tested, but amend the note's refresh-point list to name `apply_journal` (a journaled load refreshes twice, not once), and ideally reset `span_frames` on a load so a loaded piece actually shows its whole length as the note claims.

## Process note, for the record

This slice's commit was authored by two runs of the same model in one checkout: the host half was
written by the driving session after a delegated subagent appeared stalled, and the subagent — which
was in fact alive — then yielded, removed its overlapping host edits, and completed the iced shell,
the note and the dependency change on top. The subagent reported the collision plainly, and its
defect report (`from_script`'s loop refreshed per replayed command) was correct and is fixed in the
commit it landed. Both halves carry the same author identity and trailer, so attribution is intact;
the lesson is that a delegated writer must be reachable before the Lead takes its files over.
