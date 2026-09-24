# Reviewer gate (stand-in) — alpha slice D2 (tracks), GLM-5.3

> Research input, 2026-09-24. Co-worker: **GLM-5.3** (`zai/glm-5.3`), read-only, standing in for the
> designated gate **Kimi K3** (unavailable: its 5-hour API window was exhausted; the 256 k-context id
> is refused by the session's route allowlist — see
> [`2026-09-23-kimi-gate-alpha-slices.md`](2026-09-23-kimi-gate-alpha-slices.md)).
> Scope: slice D2 (add / rename / delete / reorder tracks), working tree at the time —
> `RenameTrack`/`MoveTrack` in `crates/media`, their codec and registration, the host's parser and
> formatter, the four workflow actions, and the TUI's `a`/`R`/`D`/`{`/`}`. The reviewer was told to
> attack the claims, cite file:line, separate verified from inferred, and end with a verdict.
>
> **Disposition: `merge with changes`; both should-fix findings were real and are fixed, and all six
> notes are dispositioned.**
>
> 1. **`a` announced a mixer channel that may not exist** (should-fix) — the value op is permissive,
>    but `track ti` feeds `ch{ti}` and the mixer's width is a mount parameter, so adding a fifth
>    track to a four-channel mixer printed "4 is its mixer channel" and then rendered nothing (the
>    transport stopped at the next play, with the reason only in `last_error`). Fixed: the status
>    names the mixer's width and the command that widens it. Regression coverage: the TUI's track
>    test exercises the add path; the narrow-mixer message is a status-line branch.
> 2. **The note's end-to-end claim had no committed test** (should-fix) — the host round-trip test
>    only formats and parses; it never *applies* the ops, so nothing proved that a rename/reorder
>    survives save→load. Fixed:
>    `a_renamed_and_reordered_session_reloads_in_order` saves a session whose log holds
>    `rename_track t0 lead` + `move_track lead 1`, loads it, and asserts the order and the clip's
>    home. (This is the second gate in a row to catch an over-claimed verification — a claim in a
>    note must name a test that exists.)
> 3. **`valid_track_id` under-delivered against its own doc, and `AddTrack` did not validate**
>    (note) — a name with a leading `@`, a leading `snap=`, or a `#` cannot round-trip through the
>    text format, yet the first draft's validation accepted them; and `AddTrack` had no validation
>    at all, so the *library* could create a track the save's self-check would then refuse to save
>    (every save, not a corrupt one). Fixed: the validation covers the reserved shapes and **both**
>    ops use it; the media test covers the refusals, including on `AddTrack`.
> 4. **The note's `add_track` example was wrong** (note) — the *host parser* cannot express a
>    two-word track id (arity and token stripping refuse it); the hole was library-level. Corrected
>    in the note, and closed by 3.
> 5. **Inconsistent no-arrangement behaviour** (note) — `a`/`R` explained themselves, `D`/`{`/`}`
>    returned silently. Fixed: all four say what is missing.
> 6. **A prefilled prompt lost its prefill on a history walk**, and Enter on an unfinished prefill
>    produced a raw arity error (note) — fixed: the prompt remembers what it was opened with (so
>    walking back to the live line restores it) and an unfinished prefill gets "needs the new value
>    after the last space".
> 7. **`active_track` is not restored by undo** (note) — recorded as deliberate: the cursor is not
>    document state, so restoring it would need the shell to remember a track id per gesture.
> 8. **`Action::TrackMove` sat next to `Action::MoveTrack`** (note) — a real readability trap
>    (clip move vs track reorder). Renamed to `ReorderTrack`.
>
> Reviewer-verified (recorded because they are the slice's load-bearing claims): the two ops'
> semantics exactly as claimed (in-place rename keeps index and clips; absolute move carries clips,
> own-index is a no-op, unknown/out-of-range refused); codec + `ALL_OPS` registration, so the engine
> handlers exist; strict parser arms with the `usize` guard and the inverse formatter; **replay
> determinism** (undo/redo/seek replay the full history in order; an absolute index and a
> value-based rename are order-deterministic; groups fold on a snapshot so `D` is all-or-nothing and
> one journal entry); the save self-check (a lossy session is refused, never written); `wire_arranger`
> re-deriving every `ch{ti}` by position, so no stale index survives a reorder; and that the
> clipboard/selection hold values rather than track indices, so a delete leaves nothing stale.

---

## Verbatim review

Adversarial review of D2 (tracks: add/rename/delete/reorder) — read-only, all tests run green.

## Verdict
merge with changes

## Findings

1. should-fix — `a` announces a mixer channel the track may not have, and the failure is deferred. `add_track` (spikes/tui-shell/src/main.rs:1096-1105) reports success + "`{index}` is its mixer channel" whenever the host op applied, but the channel limit is only enforced at render time in `wire_arranger` (crates/host/src/lib.rs:1201-1208: "track '{}' has no mixer channel ch{ti}"), and the shell mounts `channels=4` (main.rs:63). Scenario: a session with 4 tracks, press `a` → "added track t4 — … 4 is its mixer channel", press play → the pump's render fails, the transport is stopped and the error only surfaces via `snap.last_error` (crates/host/src/live.rs:393-400; rendered at main.rs:2669/2738). The success message is factually wrong and the breakage is indirect. Fix: check `outcome.mixer_channels` before/after announcing, or clamp/refuse the add past the mounted channel count (the value op itself may stay permissive).

2. should-fix — the note's headline verification claim has no committed test. The note (.agents/notes/implemented/feature/2026-09-24-tracks-in-the-arrangement.md:29-32) claims "verified end to end (a journal with `rename_track t0 lead` and `move_track lead 1` reloads as `t1`, `lead` …)", but the only host test touching the new ops is the format/parse round-trip (crates/host/src/lib.rs:3560-3575), which never *applies* them; no save→load or journal-recovery test covers rename/move. The apply/replay side is exercised only indirectly via the TUI test's undo. Per the note convention ("an implemented note states shipped reality"), either add a small save/load-with-reorder test or soften the claim to what the suite proves.

3. note — `valid_track_id` under-delivers vs its doc comment (crates/media/src/timeline.rs:201-206). "One whitespace-free word" does not guarantee `host v1` parseability: a name starting with `@` (at-frame token), `snap=…`, or containing/starting `#` (comment strip) cannot round-trip — all stripped in `parse_arrange_line` (crates/host/src/lib.rs:2609-2630). Unreachable through the host parser (arity errors refuse them by accident), but the engine API (`Timeline::apply`) can set such a name via RenameTrack, after which the save's self-check (lib.rs:722-736) refuses every save — an unsavable session, not a corrupt one. Consider rejecting reserved-token prefixes in `valid_track_id`.

4. note — the note's "Still open" add_track example is wrong (note lines 82-85): a live `arrange add_track two words` is an arity(2) parse error (lib.rs:2527-2530), and `add_track @`/`snap=x` are refused by token stripping — the host cannot express an invalid id at all. The hole is real only at the media-library level (direct `Timeline::apply`), which is worth saying accurately in the note.

5. note — inconsistent no-arrangement behavior: `add_track` and `rename_track_prompt` set a status when there is no arrangement (main.rs:1097-1099, 1113-1115), but `delete_track` and `move_track` `return` silently (main.rs:1126-1128, 1153-1155). Cosmetic.

6. note — the prefilled rename prompt is destroyed by a history walk: Up replaces the prefill with the last history entry and Down past the end yields a blank "live line", not the prefill (`history_step`, main.rs:1827-1842; `rename_track_prompt` sets only `history_at`, main.rs:1118-1119). Also Enter on the unmodified prefill closes the prompt and shows a raw arity error ("expects 3 words, got 2") — safe (nothing applied/logged) but unfriendly.

7. note — `active_track` is not restored by undo: after `u` on a move/delete the highlight stays at the old index (only clamped in `take_arrangement`, main.rs:507-511), so it can sit on a different track than the one that moved/deleted. Cosmetic.

8. note — `Action::TrackMove(i32)` (track) coexists with the pre-existing `Action::MoveTrack(i32)` (clip) in workflow (crates/workflow/src/lib.rs:185-191 vs 176-177 area); command_names differ (`move_track` vs `move_clip_to_track`) so behavior is right, but the near-duplicate names are a readability trap.

## Verified
- media: RenameTrack/MoveTrack semantics exactly as claimed (in-place rename keeps index+clips; taken/whitespace/empty/unknown refused; absolute move carries clips, own-index no-op, out-of-range/unknown refused) — timeline.rs:287-320 + test :646-733. `encode_op`/`decode_op`/`ALL_OPS` registered; `register_handlers` iterates ALL_OPS generically, so the engine handlers are covered (clip_editor.rs:378-399).
- host: parser arms with strict arity, `usize::try_from` guard for the index, negative/`snap=` on move_track refused; `format_arrange` inverse matches; grammar docs updated; both ops in the round-trip list (format→parse equality only, see finding 2).
- Replay determinism: undo/redo/seek rebuild by replaying the full history in order (lib.rs:1556-1600, 1602-1635); absolute MoveTrack index and value-based RenameTrack are order-deterministic; a rename-then-move of the new name is just sequential ops. Groups are folded on a snapshot first (execute_group, lib.rs:481-540) so the D gesture (clip deletes + remove_track) is all-or-nothing, one history entry, one journal entry with `group begin/end`; journal recovery drops torn entries (apply_journal). Save self-check compares re-parsed text to history before writing, so a lossy session is refused, never written (lib.rs:722-736).
- Mixer mapping: `wire_arranger` re-derives all arranger nodes from the value on `arrange_dirty` (set on every arrange apply), renumbering `ch{ti}` by position, clearing old nodes/underrun counters (lib.rs:1177-1258) — no stale index survives a reorder except the shell's cosmetic `active_track` (finding 7). Clipboard/selection hold values (frames, pool source ids), not track indices, so a delete leaves nothing stale that bites on paste.
- workflow: actions, descriptions, command_names, and keymap rows as claimed; vocabulary test drives all four names through real host lines; keymap tests (no double binding, help generation) pass.
- TUI: `a` mints first free `t{n}` from the live lanes and activates on success only; `R` prefill is exactly `arrange rename_track <track> ` and goes through `run_prompt`→`parse_script` (Esc cancels, Enter-with-no-name is a refused arity error, nothing logged); `D` builds `delete` per clip + `remove_track` as one group, reports the plural-correct count, decrements active index, handles no-arrangement/out-of-range/only-track (clamped to 0); `{`/`}` edge-guard before sending, keep the moved track active. The TUI test covers add/undo, rename-to-completion, both move edges, and delete+undo of track with 2 clips.
- `cargo test --workspace`: all green (media 15+18+19, host 30(+1 ignored)+85(+2 ignored), workflow 9, tui-shell 14+4+4+8… all passing).
- iced-shell: new actions fall into the catch-all that reports "`{}` needs the timeline" (spikes/iced-shell/src/main.rs:446-451) — the note's claim holds.

## Not verified
- `spikes/tui-shell/scripts/pty-check.sh` was not run (interactive PTY path); the real-audio pump path was reasoned about from code, not exercised.
- No fuzz/property test over random rename/move sequences vs replay (reasoned determinism only).
- The note's manual end-to-end journal reload was not reproduced (finding 2 — that is the point).
