# Reviewer gate (stand-in) — alpha slice D1 (the clipboard), GLM-5.3

> Research input, 2026-09-24. Co-worker: **GLM-5.3** (`zai/glm-5.3`), read-only, standing in for the
> designated gate **Kimi K3** (unavailable: its 5-hour API window was exhausted; the 256 k-context id
> the owner suggested is refused by the session's route allowlist — see
> [`2026-09-23-kimi-gate-alpha-slices.md`](2026-09-23-kimi-gate-alpha-slices.md)).
> Scope: slice D1 (copy / cut / paste / append), working tree at the time — `Placed.source_id`, the
> four workflow actions/keys, the clipboard + paste in `spikes/tui-shell`, and the tests. The reviewer
> was told to attack the claims, cite file:line, separate verified from inferred, and end with a
> verdict.
>
> **Disposition: `merge with changes`; both must-fix findings and both should-fix findings were fixed
> before merge, with tests.**
>
> 1. **The fade guard did not guarantee the model's `fade_in + fade_out <= src_len`** (must-fix) —
>    keeping a copied fade on one side while giving the bare side a 64-frame guard breaks the rule for
>    a tight pair (the gate's example: `src_len = 100`, copied `fade_in = 60` → `60 + min(64, 50) > 100`;
>    an even sharper one: a legal `fade_in = src_len`). The host refuses the group **whole**, so the
>    paste silently did nothing. Fixed: each bare side is capped at `src_len − the other side`, in a
>    fixed order (in, then out), so the pair is legal for every combination; the previous note claim
>    ("capped at half the clip") was wrong and is corrected. Regression test
>    `a_tight_copied_fade_pair_pastes_legally` (a 100-frame clip with a 60-frame fade-in, and a
>    full-length fade-in).
> 2. **A refused group still announced success** (must-fix, compounding 1) — `App::command` recorded
>    the refusal in the status line and then `paste` overwrote it with "pasted N clips …" *after*
>    burning its minted ids, so the user saw success, nothing appeared, and a following `u` undid the
>    previous edit. Fixed: `command`/`arrange_group` return whether the host applied the command, and
>    `cut`, `paste` and `trim_to_selection` set their own status only then. Test
>    `a_refused_gesture_never_claims_success`.
> 3. **`P` (append) reported a snap that never happened** (should-fix) — the status compared the
>    *playhead* with the append target (the track's last clip end). Fixed: the snap note is emitted
>    only for the playhead branch.
> 4. **The clipboard was not cleared on load, contradicting the note** (should-fix) — and a paste
>    naming a source the new session's pool lacks was logged by the media layer (which does not check
>    the pool) and then dropped by the panel. Fixed by keeping the clipboard across a load (a value,
>    like a DAW's) and having the shell hold the pool's source ids: the paste is refused with the
>    missing id named, *before* minting anything. The note's "a reload starts empty" is corrected.
> 5. **Multi-clip paste was untested** (note) — it works only because group members carry no `@frame`,
>    so the host's "one frame per group" rule is satisfied trivially. A multi-clip test now covers
>    spacing preservation and one-undo-removes-both.
> 6. **A one- or two-frame clip gets no guard at all** (note) — true (`min(64, src_len/2) == 0`); the
>    note overclaimed by omission and now says so.
> 7. **A pre-existing flake outside the diff** (note) — `host::tests::a_take_records_into_the_pool_and_plays`
>    failed once under a parallel workspace run: its fixed 120 ms drain sleep is a race on a loaded
>    machine. Fixed as its own commit (poll the demux instead of sleeping), not smuggled in here.
>
> Reviewer-verified (recorded because they are the slice's load-bearing claims): the selection filter's
> half-open boundary semantics and zero-width fallback; `cut` deleting exactly what `y` took; the minted
> id seeding sound across double-paste, undo, redo, reload and a saved session holding `paste.N`; a
> user-typed `paste.1` collision refused whole; the missing-track guard; the origin normalisation (no
> clip precedes the minimum); mixer focus making the clipboard keys no-ops; iced reporting the new keys
> through its catch-all; and that **nothing clipboard-ish reaches the log** (a copy logs nothing, a cut
> logs only `delete`, a paste only `add_clip`), so replay stability rests on the existing canonical
> serialisation plus the ids being in the log.

---

## Verbatim review

Adversarial review of D1 (clipboard) complete. Verdict: **merge with changes** — one real must-fix bug pair the tests miss.

## Verdict
merge with changes

## Findings
1. **must-fix** — The micro-fade cap does NOT guarantee the model's `fade_in + fade_out <= src_len` when a copied fade is kept on one side. `paste` (spikes/tui-shell/src/main.rs:1207-1216) uses `micro = MICRO_FADE.min(clip.src_len / 2)` and keeps a nonzero copied fade on the other side unchanged. media rejects `fade_in + fade_out > src_len` (crates/media/src/timeline.rs:206) and the host's group validates on a probe snapshot and refuses whole (crates/host/src/lib.rs:508-517). Scenario (VERIFIED by code path): a clip with `fade_in = src_len, fade_out = 0` (legal; reachable via `f` with the playhead at the clip end, or a typed `set_clip_fade`/`add_clip`), or more broadly any copied fade > `src_len - min(64, src_len/2)` (e.g. src_len=100, fade_in=60, fade_out=0 → 60+50 > 100) — `y` then `p` mints `add_clip` with an invalid fade pair and the whole paste is silently refused. The note's claim "capped at half the clip so the model's rule cannot be violated" is false for the kept-fade combination. Fix: cap each side at `src_len - other_side` like `fade_to_playhead` already does (main.rs:1492-1497).

2. **must-fix** (compounds 1) — `paste` reports success even when the host refused the group. `App::command` records the refusal in `status` ("arrange refused: …", main.rs:588-598), but `paste` then overwrites `status` unconditionally with "pasted N clips at … (one `u` undoes the paste)" (main.rs:1230-1236), after already burning the minted ids (`next_paste += len` at main.rs:1229). Scenario: finding 1's clip — the user sees "pasted 1 clip", nothing appears, and a following `u` undoes the *previous* edit (or no-ops), which looks like data loss. `cut` has the same pattern (status set after `arrange_group`, main.rs ~1150) though a delete of a just-copied clip is hard to refuse. Fix: have `arrange_group`/`command` return success and only then set the success status.

3. **should-fix** — `P` (append) status lies about snapping. `paste` appends `snapped_note(self.snap.frame, target, grid)` (main.rs:1233-1235) where `target` is the track's last-clip end, not a snapped playhead; `snapped_note` (main.rs:2532-2540) has no grid-on check and always prints when raw≠snapped. Scenario (VERIFIED): their own `cut_and_append` test state — grid off, playhead 10 000, append target 192 000 — would render "appended 1 clip at 192000 (snapped from 10000 on the off grid)". The test only asserts the frame, not the status, so it ships. Only emit the note for the non-append branch.

4. **should-fix** — The clipboard is NOT cleared on load, contradicting the note ("a reload starts empty", note line 75). `open_script` (main.rs:317) and the session `load` path mutate the App in place; `clipboard`/`clipboard_origin` persist (only the two App constructors reset them, and those run at startup). Consequence: paste after a load names a `source_id` the new pool may lack; media's `AddClip` checks only the track and field invariants (timeline.rs:271-277), so the op is logged, then `Arrangement::from_host` silently *drops* the clip (timeline.rs "no pool source with this id" error path) — a clip that exists in the session but is invisible in the panel. Not the "loud failure" the note claims. Clear the clipboard on load, or keep it and surface the reconcile error loudly.

5. **note** — Multi-clip paste is untested and rests on a subtle host property. All three new tests copy exactly one clip. A multi-clip paste works only because the `add_clip` lines carry no `@frame`, so every group member's `at_frame` is `None` and the host's "members must share one frame" check (host lib.rs:496-505) passes trivially (VERIFIED by reading; INFERRED at runtime). If the host ever requires a real frame for groups, every multi-clip paste breaks. Also the relative-spacing preservation and the "one `u` removes N clips" gesture for N>1 are proven only by inspection.

6. **note** — `src_len = 1` clips get no click guard at all (`micro = min(64, 0) = 0`), so "new boundaries get a micro-fade" is false for 1-frame clips (harmless, but the note overclaims). Zero-length clips cannot exist (timeline.rs:191), so the selection filter's zero-length case is moot.

7. **note** — Pre-existing flake, unrelated to this diff: `host::tests::a_take_records_into_the_pool_and_plays` failed once under `cargo test --workspace` (parallel) and passed on rerun/isolation; the host crate is untouched by the change. INFERRED timing-related.

## Verified
- Claim 1 (Placed.source_id) — timeline.rs diff, populated in `from_host`; test asserts the id travels.
- Claim 2 (workflow actions/keymap/command_name) — keys y/c/p/P are free (no other `Key::Char` binding in workflow KEYMAP); the vocabulary test drives the samples through `host::parse_arrange_line` (workflow lib.rs:624-660), so `delete`/`add_clip` provably parse.
- Claim 3 core paths: `y` scope (half-open selection intersection `at < to && end > from` — correct at both boundaries), zero-width fallback to `clip_at`, `c` = same scope + one `delete` group, `p`/`P` targets, minted-id seeding (`max_paste_id` scans **all** lanes at paste time; sound across double-paste, undo (counter never decreases), redo, reload, and a saved session holding `paste.N`; the host enforces global clip-id uniqueness, timeline.rs:275); user-typed `paste.1` collision refused whole with a clear message; paste onto a missing track guarded both shell-side and host-side ("no track"); origin = min start so no clip precedes it; `clip_at` start is always ≤ playhead; mixer focus makes y/c/p/P no-ops via `timeline_key` (consistent with x/d); iced reports the new keys via its catch-all "`…` needs the timeline" arm (iced main.rs:445-451).
- Determinism: nothing clipboard-ish reaches the log — yank logs nothing, cut logs only `delete` lines, paste logs only `add_clip` lines; replay stability rides on the host's canonical serialization (existing round-trip tests) and the ids being in the log.
- Tests: workflow 9 passed, tui-shell 38 passed, workspace green except finding 7's flake.

## Not verified
- `spikes/tui-shell/scripts/pty-check.sh` not run (real terminal path); live audio paths not exercised.
- Runtime reproduction of findings 1-4 was established by code-path reading plus arithmetic, not by executing the failing gesture (read-only mandate; no test files could be added).
- pty/rendering behaviour of the new status strings.
