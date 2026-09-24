# Reviewer gate — slice A1 (compound gestures): attempted, did not return

> Research input, 2026-09-23. Co-worker: **Kimi K3** (the designated reviewer gate in the
> [co-work note](../../.agents/notes/implemented/process/2026-08-27-model-co-work-routing.md)), read-only,
> over slice A1 of the [alpha plan](../../.agents/notes/proposed/architecture/2026-09-23-alpha-finish-line.md):
> `HostCommand::Group` + the host history as entries (`Vec<Vec<HostCommand>>`) + `group begin`/`group end`
> in the text form + the TUI's first gesture (trim-to-selection).
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
