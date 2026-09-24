# Reviewer gate — the alpha plan's architectural slices (A1, A2): attempted, did not return

> Research input, 2026-09-23. Co-worker: **Kimi K3** (the designated reviewer gate in the
> [co-work note](../../.agents/notes/implemented/process/2026-08-27-model-co-work-routing.md)), read-only,
> over the [alpha plan](../../.agents/notes/proposed/architecture/2026-09-23-alpha-finish-line.md)'s
> architectural slices:
>
> - **A1 — compound gestures**: `HostCommand::Group` + the host history as entries
>   (`Vec<Vec<HostCommand>>`) + `group begin`/`group end` + the TUI's first gesture (trim-to-selection).
> - **A2 — session save/open**: a session directory (`session.txt` = the log, `pool/`, a journal),
>   the text formatter that inverts the parser, `SessionRate`/`Save`/`Load`, journal recovery.
>
> **Outcome: no review returned.** Three routes, all failed or stalled:
>
> 1. **`opencode-go/kimi-k3`** via the subagent API — failed before finishing (twice, including a retry
>    with a shorter brief). The owner confirms the `opencode-go` provider is currently broken.
> 2. **`kimi -p "<brief>"`** (the CLI route in the co-work note) — the process planned its pass and then
>    sat at ~0 % CPU for minutes with no output; `--prompt` cannot be combined with `--auto`, so its tool
>    approval had no TTY to answer. Killed.
> 3. **`kimi --auto` inside a pty** (`script -qec`, the fix recorded in the alpha-scope archive): the
>    brief *was* received (context reached 30 % / 75.6k tokens, "Never Ask" mode, thinking high), then the
>    output stopped growing and CPU went idle — a stall on the model/tool side, not the harness.
>
> `kimi doctor` reports the CLI configuration valid, so this is a runtime/backend problem (the same
> provider trouble that takes out `opencode-go`), not a local misconfiguration.
>
> **Disposition: the slice is merged without the gate, and the gate stays owed.** The driver's own
> pre-merge checks: 24 workspace test binaries green (host 17 unit tests including the four new gesture
> tests), clippy and fmt clean, TUI 26 tests, and a self-review of the failure modes the gate was asked
> about (clone-then-apply divergence, partial application, the `replay_to` whole-entry skip, parser
> folding, and whether grouping touches the engine log). Those findings are in the
> [A1 note](../../.agents/notes/implemented/architecture/2026-09-23-compound-gestures-one-undo-step.md).
> When Kimi returns, this file is where its answer belongs, and A1 is the first commit it should read —
> followed by A2 (session persistence), which touches the same history shape.
>
> **Fix for the next attempt:** the pty route works up to the submit (a multi-line paste needs an
> explicit `\r` after it) and then must ride out the backend stall — try at an off-peak hour, or use a
> different cross-vendor model for the gate if the owner prefers (GLM-5.3 stood in for the *plan* review
> and is independent of this repo's DeepSeek author, so it is the natural stand-in).

## The brief that was submitted

```
You are the independent reviewer gate for ONE commit in a Rust audio project. Adversarial, concrete,
read-only: do not modify any file.

Repo: /home/geir/Projects/sound-arranger. The change under review is "compound gestures": one editing
gesture = one undo step.

Read these, then judge:
- crates/host/src/lib.rs — `HostCommand::Group`, `HostSession::execute_group`, `history`/`redo` (now
  `Vec<Vec<HostCommand>>` / `Vec<(usize, Vec<HostCommand>)>`), `replay_to`, `undo`, `redo`, `can_undo`,
  `process`, and `parse_script` (the `group begin` / `group end` markers).
- crates/media/src/timeline.rs — `Timeline::apply` (the pure transform the group is validated against).
- crates/media/src/clip_editor.rs — `ClipEditor::apply` / `snapshot`.
- spikes/tui-shell/src/main.rs — `arrange_group` and `trim_to_selection`.
- The tests: `a_group_is_one_undo_step`, `a_refused_group_changes_nothing`,
  `a_group_is_one_frame_of_arrangement_ops`, `a_grouped_script_round_trips_and_logs_the_same_events`.

Claims to check, one by one (say whether each holds, and why or why not):
1. All-or-nothing: the group is folded over a `Timeline` clone before any member is applied for real.
   Can the clone-then-apply pair diverge, or can a member fail after the probe succeeded?
2. History as entries: any regression for the single-command case?
3. `replay_to` skips an entry when its first command's `at_frame > target`: is that still correct for
   one-element entries and for `None` frames? Can a seek produce a state a forward replay would not?
4. Parser: unmatched/nested/empty groups, comments/blank lines inside a group, the `mark`/`drain` fold.
   Any line kind that slips out of the group or is mis-folded?
5. The engine's own log is claimed unchanged by grouping, so byte-identical replay/bounce is unaffected
   by construction. Anything in the code that contradicts that?
6. Is a recursive `Group` command the right shape, or is there a materially better representation given
   the rules (the log is the document; replay is deterministic; the render path never allocates)?

Name the single most likely bug the tests above would NOT catch, and one thing you would refuse to merge
without. End with `VERDICT: merge` / `VERDICT: merge with changes: …` / `VERDICT: do not merge: …`.
```

## Slice A2 — session save/open: also stalled

The same three routes were tried for A2 (the brief is in the transcript; `opencode-go` refused to
finish, `kimi -p` parked on an approval, and `kimi --auto` inside a pty received the brief and then
stalled at 2 % context / 15 KB of output with no growth). Kimi was killed after several minutes.

**Root cause, from the owner (2026-09-23):** the CLI's device login is now answered with
`403 Forbidden` (`kimi-code-cli/2.0.2`, Login Device) — a server-side rejection of the client, not a
quota error — and the API side has **hit its 5-hour limit** (reset ~1 h 42 min later). The environment
also has the owner's API key, so the API route is the one to retry once the window resets; the CLI
route needs the 403 resolved (a client update, or the key route instead).

**Stand-in:** because two architectural slices had now gone ungated, the gate was handed to
**GLM-5.3** (cross-vendor to this repo's DeepSeek author, and the model that stood in for the plan
review). Its answer is archived in
[`2026-09-23-alpha-slice-gate-glm-standin.md`](2026-09-23-alpha-slice-gate-glm-standin.md) with a
disposition. The designated gate stays owed for both slices when Kimi returns.

## Addendum — what to do when Kimi returns

1. Run this file's A1 brief and the A2 brief (both in the transcript) against **Kimi K3 via the API**
   (`opencode-go/kimi-k3`) once the 5-hour window resets — the CLI's device login is 403ing, so the API
   is the route. Archive the answers here, verbatim, with a disposition.
2. Prioritise in this order: (a) the A2 journal-recovery rule (the mid-gesture cut — a self-review found
   and fixed it, so an independent check of that fix matters most), (b) A1's all-or-nothing probe/apply
   pair, (c) the A2 text round-trip completeness.
3. If Kimi is still unavailable, the GLM stand-in review is the recorded substitute; it is cross-vendor
   to the author, but it is *not* the designated gate, and both slices stay owed.

## Slice A4 (stereo material) — refused again, 2026-09-24

The A4 brief (the same adversarial brief as the GLM stand-in gate for that slice) was sent to
`opencode-go/kimi-k3` on 2026-09-24. The run **failed before it finished and left no closing message**
— the same failure shape as A1/A2 (the route accepts the request and never returns a result). No
review output was produced, so nothing is archived here.

This keeps the designated gate **owed** for A1, A2, A3 and now A4. (The owner's status read at
2026-09-24 13:03 local: 5-hour window 100 % used, resetting 14:09; 7-day window 20.94 %.) The GLM-5.3 stand-in for A4 is
archived in
[`2026-09-24-alpha-slice-gate-a4-glm-standin.md`](2026-09-24-alpha-slice-gate-a4-glm-standin.md)
with a disposition; the substitution is recorded there, not here (this file stays the record of the
Kimi route's state).

**What the owner can unblock it with (2026-09-24):** the API route is the one to try, and the account
now reports the **5-hour usage window at 100 %** (reset 2026-09-24 14:09 local) with the 7-day window
at ~21 % — so the failure is the short window, not the key. The owner also relays that the provider
wants the **256 k-context** variant, i.e. the model id `k3-256k` (the DSH catalog advertises
`opencode-go/kimi-k3`; an unlisted id is worth trying since adapter membership is advisory). Retry
after 14:09 with that id, for the A4 brief first and then the owed A1–A3 retro-gate. The CLI's device
login still answers `403 Forbidden`, and `kimi -p` conflicts with `--auto`.
