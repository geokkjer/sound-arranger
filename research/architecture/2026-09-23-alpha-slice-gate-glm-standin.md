# Reviewer gate (stand-in) — alpha slices A1 + A2, GLM-5.3

> Research input, 2026-09-23. Co-worker: **GLM-5.3** (`zai/glm-5.3`), read-only, standing in for the
> designated gate **Kimi K3** (unavailable: `opencode-go` fails and the kimi CLI's device login returns
> `403`, with the API's 5-hour window exhausted — see
> [`2026-09-23-kimi-gate-alpha-slices.md`](2026-09-23-kimi-gate-alpha-slices.md)).
> Scope: slice A1 (compound gestures, commit `ed1e83e`) and A2 (session save/open + journal, working
> tree at the time). The reviewer was told to attack the claims, cite file/type names, separate verified
> from inferred, and end with a verdict.
>
> **Disposition: `merge with changes`, and all four A2 findings were fixed before A2 merged.** The four
> were real, verified in code, and not caught by the tests:
>
> 1. **`replay_to` dropped `session_dir`** (`*self = rebuilt` carried only `playing`/`redo`), so autosave
>    **silently stopped** after the first undo, redo or seek on a saved session. Fixed: the rebuild now
>    carries `session_dir`, `journal_error` and `last_recovery`; regression test
>    `autosave_survives_an_undo_and_a_seek`.
> 2. **`save` re-pointed the live pool at the copy but left the history naming the original**, so a
>    replay after a save re-adopted the old path — and failed outright once it was moved or deleted.
>    Fixed: `rebase_pool` rewrites the `pool` command in the history (recursing into gestures), so
>    history, script and the live session agree; regression test `an_undo_after_a_save_uses_the_session_pool`
>    (the original pool is deleted before the seek + undo + bounce).
> 3. **`journal_append` disabled the pool rewrite while its comment claimed otherwise**, and a `pool`
>    line in a journal could make a moved session refuse to open. Fixed: the journal records the entry's
>    own paths, and `apply_journal` now **drops and reports** a refused command
>    (`JournalRecovery::refused`/`refused_reason`) instead of failing the load; regression test
>    `a_stale_journal_is_dropped_not_fatal`.
> 4. **The save's rename→journal-reset window**: a crash in between leaves a stale journal that replays
>    on the new baseline (duplicate ids). Fixed by the same tolerance (the stale commands are refused and
>    reported, not fatal); the script is also `sync_all`ed before the rename, and the ordering is
>    documented.
>
> Two further refinements from the review: the save now **verifies its own output** (parse it back and
> compare to the history, refusing a lossy save — a whitespace id or a region play is refused rather than
> written), and `copy_pool` skips a file only when size *and* mtime match. Regression tests:
> `a_save_that_cannot_round_trip_is_refused`, plus the earlier `a_saved_session_can_be_moved`.
> A1 needed only a comment fix (the fold-then-apply invariant was stated as unconditional).
>
> Open from the review, recorded as debt: no `fsync` of the journal (process-crash safe, not
> power-loss safe); the parser is not arity-strict for `pool`/`load`/`save` (the save's self-check is
> what catches a truncated path); a hand-edited malformed journal line still fails the whole load.

## The review, verbatim

# Independent review — Slice A1 (commit `ed1e83e`) and Slice A2 (working tree)

**Scope note.** Slice A1 is the committed `ed1e83e`; Slice A2 exists only as uncommitted changes to `crates/host/src/lib.rs` and notes. I reviewed read-only and did not run the test suite (running `cargo test` writes to `target/`, which the read-only constraint forbids); everything below is from reading the code. Line numbers refer to the current working-tree `lib.rs`.

## Slice A1 — compound gestures

**(i) All-or-nothing fold — verified, with one overclaim in the comment.** `execute_group` (463–513) folds members over `editor.snapshot()` and refuses on the first `Err` before touching the live value. The real-apply loop then calls `ClipEditor::apply` per member, which mutates the timeline *then* calls `engine.arrange_logged` (`clip_editor.rs` 428–438) — so a mid-group engine failure would leave a half-applied gesture. In practice the window is closed: `arrange_logged` fails only via `validate_op` (`render.rs` 505), i.e. an unregistered handler or a non-finite `F32` field; `ensure_editor` registers all handlers first, and the only `F32` fields in arrange ops are gains, which `Timeline` itself validates finite (`timeline.rs` 197, 448). So the fold and the real apply cannot diverge *today* — but the invariant lives implicitly in three files. The comment "cannot diverge" is an inference from the field types, not an enforced property; a future op with an `F32` field the timeline doesn't validate reopens the hole silently.

**(ii) Engine log untouched — verified.** Members go through `arrange_logged` exactly as loose `Arrange` commands do; `a_grouped_script_round_trips_and_logs_the_same_events` asserts equal event counts and equal values, and the audio path is unaffected since groups never touch the scheduler.

**(iii) `replay_to` skip — verified correct.** The per-entry skip (1304–1310) tests `entry.first().and_then(at_frame)`; groups are guaranteed single-frame by validation, one-element entries use their own `at_frame`, and `None`-frame entries are never skipped (correct: they apply "now"). `at == frame` is applied, `at > frame` skipped — right boundary. One subtlety: the skip uses the *command's* `at_frame`, while the engine log records the *clock* frame at apply time; these can differ for a group submitted with a past `at_frame` on an advanced clock. Replay is from history, not the engine log, so this is latent, not live.

**(iv) undo/redo/can_undo single-command regression — none found.** The rposition-over-entries scan (1330) degenerates to the old behaviour for one-element entries; `redo`'s `pos.min(len)` clamp is the same as before; `redo.clear()` on new state is unchanged and correctly replayed (`replay_to` restores the taken `redo` after the loop, 1317).

**(v) Recursive `Group` — acceptable.** Nesting is refused twice (parser 1803; `execute_group` 474), so recursion is unreachable from any public path; the recursive arm in `format_command` (1559) is effectively dead but harmless.

## Slice A2 — session directory + journal

**(i) Round-trip completeness — partially false.** `fmt_f32` uses `{:?}`, which is shortest-round-trip for f32 (bit-for-bit for all finite values including subnormals; a NaN's payload is not preserved — `set_param`'s parser accepts `NaN`, a nit). `add_clip`'s optional `loop_len` round-trips via `filter(|v| *v != 0)` (2110) and the formatter's conditional append — correct, including `Some(0)` collapsing to `None`, which is canonical anyway. Actions correctly have no form. **But:** `play`'s form (1545–1553) emits only `path` and `channel` — a `Play` with non-zero `start`/`len` parses back as whole-file. And the grammar is whitespace-split with no quoting: a pool path, bounce path, or track/clip id containing a space produces an unparsable script. The test only uses space-free paths and `start: 0, len: 0`, so it passes while the general claim is false.

**(ii) `save` crash-safety/portability — this is where the real bugs are.** Four findings, all verified in code:

1. **`replay_to` drops `session_dir`** (1292–1319): the rebuilt session gets only `playing` and `redo`; `*self = rebuilt` wipes `session_dir`, `journal_error`, `last_recovery`. After any undo, redo, or `transport seek` on a saved session, `journal_append` (647) returns early — **autosave silently stops for the rest of the process lifetime**. No test journals after an undo/seek; this is the single most likely bug the tests would not catch.
2. **`save` re-points the live pool but not the history** (547–554): `set_pool(target_pool)` updates `pool_dir`, yet the `Pool` entry in `history` keeps the original absolute path. `replay_to` replays history, so undo/seek/redo *after a save* re-adopts the original pool — and fails outright if it was deleted (precisely `a_saved_session_can_be_moved`'s `remove_dir_all(&pool)`; the test just never touches the original session again). Even in place, the live session and its saved script then disagree about the pool.
3. **`journal_append`'s pool rewrite is disabled** (653): it passes `live_pool: None` while the comment claims it keeps the session's current pool. A `pool` line in a journal keeps the original absolute path; on a moved session `apply_journal` → `execute(Pool)` → `set_pool` fails, and the `Err` propagates through `load_session` — the session **refuses to open** because of an autosave line.
4. **Ordering/fsync**: the script rename (561) lands before the journal truncation (564); a crash between leaves a stale journal replayed on the new baseline — duplicate `add_clip` ids are refused and `apply_journal`'s `Err` again makes the session unopenable. There is no `sync_all`/`sync_data` anywhere, so temp+rename is atomic against process death only, not power loss. `copy_pool`'s same-size skip (1708–1714) also keeps a stale file whose content changed at equal length — minor.

**(iii) Torn-line detection — sound for every cut position; I tried to break it and failed.** A partial append can only cut the tail. A cut at a line boundary without `\n` → last line dropped (619). A cut leaving complete member lines but no `group end` → the `rposition` scan (625) truncates from the last `group begin`. An incomplete line cannot end in `\n` (it's the line's last byte), so no incomplete line survives rule one; earlier entries are always complete because appends are sequential. The mid-gesture test covers the hard case correctly. Residual risk: a *complete but malformed* line (hand edit) still fails the whole load — defensible but worth knowing.

**(iv) `SessionRate` as context — correct.** `from_script` reads the first `SessionRate` before the engine applies anything (1244–1252); zero is refused in both `from_script` and `process` (813); a differing second line errors during `process`; an equal one is a no-op. `save`'s header rate plus a history `SessionRate` line yields two identical lines — harmless duplication.

**(v) Pool rewrite vs original history path — see (ii).2/3.** Replay determinism of the *live post-save session* is the casualty: the saved script is self-consistent (relative `pool` resolved by `resolve_session_paths`, which also recurses into groups — good), but the in-memory history is not re-based, and the journal's pool lines aren't rewritten at all.

## Direct answers

- **Most likely bug the tests would not catch:** `replay_to` wiping `session_dir`, silently disabling journal autosave after the first undo/seek on a saved session. Runner-up: the stale-pool-path replay failure after `save`.
- **Refuse to merge without:** (a) `session_dir` (and `last_recovery`/`journal_error`) carried across `replay_to`, ideally with a test that saves → edits → undoes → edits and asserts the journal grew; and (b) the pool re-basing fixed coherently in all three places (`save`'s history rewrite, `journal_append`'s `live_pool`, or resolving pool paths at replay time against `session_dir`).

**Verified:** all line-level behaviour cited above. **Inferred:** no fsync → power-loss exposure; `Timeline::apply` purity (its docs claim determinism; I checked for randomness/UUID sources and found none, but did not prove it over all ops).

VERDICT: merge with changes: fix `replay_to` losing `session_dir` (autosave silently stops after undo/seek) and re-base the pool path consistently across history, journal, and save; address the save-rename→journal-reset crash window.
