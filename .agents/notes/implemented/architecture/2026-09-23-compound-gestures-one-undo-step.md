# Agent Note: compound gestures — one gesture, one undo step

Status: implemented

## Problem

Undo was per *command*: `HostSession::history` was a `Vec<HostCommand>` and `undo` dropped the last
`Arrange` op it could find. A gesture that takes more than one op therefore took more than one undo.
The first instance is `t` (trim to the selection), which emits a `trim start` and a `trim end`, so
pressing `u` after it left the clip half-trimmed — the edit the user thought of as *one action* came
apart. The alpha plan's clipboard, append, utility and stretch gestures make the same assumption
worse: without a unit, every one of them would be N undos, and a *save* file would record a gesture as
N unrelated commands.

## Decision

**One history entry = one gesture.** The host's history becomes
`Vec<Vec<HostCommand>>` (a bare state command is a one-element entry) and `redo` becomes
`Vec<(usize, Vec<HostCommand>)>`; `undo`/`redo`/`can_undo` move whole entries.

The unit itself is a **command**: `HostCommand::Group { commands: Vec<HostCommand> }`.

- **The engine's log is untouched.** Each member is still applied and logged exactly as before, so
  replay and the byte-identical bounce tests are unaffected *by construction* — grouping changes the
  host's history, not the engine's event stream. `a_grouped_script_round_trips_and_logs_the_same_events`
  asserts it: the same ops grouped and loose produce the same value, the same event count and the same
  undo behaviour differs only in step count.
- **All-or-nothing.** `execute_group` folds the members over a **snapshot of the arrangement value**
  (`ClipEditor::snapshot`, a pure `Timeline`) before applying any of them for real; a member the model
  refuses means no member is applied and nothing is logged, and the error says so
  (`gesture refused, nothing applied: …`). The value is a pure transform, so validating on a clone and
  then applying cannot diverge.
- **One moment.** Members must be arrangement ops *sharing one `at_frame`* (a group is one edit at one
  time), which is what makes `replay_to`'s skip-after-target rule safe to apply to the **entry**: a
  seek past a gesture skips it whole, never half.
- **The text form carries the structure**: `group begin` … `group end`, parsed into one `Group`
  command. Unmatched, nested and empty groups are refused loudly. A saved session therefore round-trips
  the gesture (and, later, replays it as one step).
- **The shells send it as one command.** `HostCommand::Group` is the seam; the TUI's `arrange_group`
  parses several `arrange` lines into one group, and `t` (trim to selection) is its first user.

## Evidence

- `crates/host/src/lib.rs` tests: `a_group_is_one_undo_step` (two trims → one undo restores the clip
  whole, one redo re-applies it), `a_refused_group_changes_nothing` (the second trim would empty the
  clip: nothing applied, `event_count` and the media-command counter unchanged), `a_group_is_one_frame_of_arrangement_ops`
  (mixed frames and a non-arrangement member are refused), `a_grouped_script_round_trips_and_logs_the_same_events`
  (marker parsing, value/event equality with the loose form, one-undo vs two, and the four malformed
  group forms refused).
- `spikes/tui-shell`: the visual trim test now asserts one `u` restores the clip whole and one
  `Ctrl+r` re-applies it (26 tests).
- Workspace: 24 test binaries green, clippy and fmt clean.
- Co-work gate: see *Review* below.

## Review

The plan commits architectural slices to the co-work reviewer gate (Kimi K3). This slice is a
log/history-shape change, so it was submitted — **and the gate did not return**: the `opencode-go` API
route failed twice, `kimi -p` hung on tool approval (no TTY; `--auto` cannot combine with `--prompt`),
and the pty route (`script -qec "kimi --auto"`, the documented fix) received the brief and then stalled
on the model side at 30 % context. `kimi doctor` reports the CLI config valid, so it is a
runtime/backend problem, not a local one. **The gate is therefore owed, not skipped**: the brief and the
diagnosis are archived in
[`research/architecture/2026-09-23-kimi-gate-compound-gestures.md`](../../../../research/architecture/2026-09-23-kimi-gate-compound-gestures.md),
A1 is the first commit that review should read when Kimi returns, and the driver's own pre-merge checks
(24 workspace binaries, host 17, TUI 26, clippy and fmt clean, plus the failure-mode self-review the
gate was asked about) are what stood in for it.

## Alternatives considered

- **Keep `history: Vec<HostCommand>` and mark group membership with a flag/sentinel** (e.g. a
  `HostCommand::GroupEnd` marker). Rejected: it makes the *reader* of the history responsible for
  pairing markers, and a truncated or hand-written history has no way to be validated — the entry
  list makes an unbalanced group unrepresentable.
- **Make the undo unit "all ops since the last `at_frame` change"**. Rejected: it invents a rule the
  log does not state, and it would silently merge two deliberate edits that happen to share a frame.
- **Fix it in the shell** (remember the ops a key produced and issue several undos). Rejected: the
  shell is not the owner of history — a script, the `:` prompt and (later) a paste from the iced shell
  would all take different paths, and the undo count would depend on which shell did the editing.
- **A transaction API on `HostSession`** (`begin_gesture`/`end_gesture`) that streams commands.
  Rejected for now: it needs mutable borrows across the whole gesture (awkward for a `&mut self`
  caller), and `Group` already expresses it as a value — which is what the log, the parser and the
  shells all want. Revisit if a gesture ever needs to include a *non*-arrangement command.
- **Allow mixed commands in a group** (e.g. a paste plus a `set_param`). Rejected: it breaks the
  shared-frame rule that makes `replay_to`'s whole-entry skip sound. A gesture that needs both should
  be two gestures, or the second command should become an arrangement op.

## Consequences

- A gesture is one undo step in every path — key, `: ` line, script or replay — because the unit lives
  in the host's history, not in a shell.
- `replay_to` (seek, undo, redo) now reconstructs **whole entries**, so a seek can no longer land
  inside a gesture.
- Saved sessions (the next slice) can round-trip gestures, and their journal can record one line per
  gesture rather than one per op.
- A group is arrangement-only and single-frame for now. `Paste`/`Append` need only arrangement ops, so
  they fit; if a future gesture needs a mixer change at the same moment it gets its own mechanism
  rather than widening this one.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-23.
