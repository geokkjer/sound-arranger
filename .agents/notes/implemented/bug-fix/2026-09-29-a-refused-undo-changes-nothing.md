# Agent Note: A refused undo or redo changes nothing

Status: implemented

## Problem

Undo and redo are a rebuild: `HostSession::undo` drops the most recent arrangement
entry from `history`, pushes it onto the redo branch, and then replays the
*remaining* history into a fresh session
([note](../feature/2026-09-10-undo-redo-arrangement-history.md)). The replay is
fallible — it re-opens the pool, re-reads every clip's source, re-mounts the graph
— and the ordinary reason it is refused has nothing to do with the edit being
undone: the pool directory moved or was deleted, a `play` clip's file is gone, the
script's material is not where the log says it is.

That rebuild happens **after** both stacks were already mutated, and its `?`
returned the error straight out. So a refused undo left the entry **out of the
history and on the redo branch** while `*self` was never replaced: the arrangement
value and the graph still held the edit, and the history no longer said so. The
consequences compound rather than announce themselves. The next `save` builds its
script from `history`, so it writes a session **without** the edit and the change
is lost the next time the file is opened. `can_undo` and `can_redo` now describe a
state that does not exist, so the shell's `⟲`/`⟳` lie. And the next undo does not
retry the refused one — it finds a *different* entry, so the edit the user was
undoing stays applied while another one silently disappears.

Redo had the mirror defect, and there it was strictly destructive: the entry was
popped off the redo branch and inserted into `history` before the replay, so a
refusal consumed the branch's only entry outright. There was no second undo and no
reload that could bring that edit back.

`seek_to` already stated the rule the two functions were breaking — *a refused
replay leaves the original session untouched* — and the refused live load honours
it ([note](2026-09-29-a-refused-live-load-keeps-the-session.md)) by building its
candidate and adopting it only once it built whole. Undo and redo were the two
paths that read like that sentence and did not do it.

## Decision

**Undo and redo mutate `history` and `redo` to describe the candidate session, and
put both back when the replay is refused. `Err` from an undo or a redo means "no
edit was applied", never "the edit was consumed but could not be built".**

- `undo` keeps a copy of the one entry it removed and the redo stack's length before
  it pushes. On a refused replay it truncates the redo stack back to that length and
  inserts the entry at its original index, then returns the replay's error
  unchanged. `truncate` rather than a `pop`, because a take finalized on the way
  into the rebuild clears the branch for itself (`commit_state`), and a refusal
  undoes neither that nor the entry above it.
- `redo` recovers the entry it inserted with `history.remove(at)` rather than a
  second copy, so the refusal costs one `Vec` shift and no clone.
- The copy in `undo` is **one gesture**, not a snapshot of the history: an entry is
  a handful of commands ([note](../architecture/2026-09-23-compound-gestures-one-undo-step.md)),
  so restoring is bounded by a paste, not by the length of the session.
- The replay still runs on the mutated stacks, because that is what has to be built:
  `rebuild` carries `redo` across into the adopted session, so a candidate with a
  stale branch would install one. Building the candidate first and assigning after
  would need the whole history taken by `&mut`, which is the `replay_to_kind`
  contract's business to change, not this fix's.
- The error is passed through untouched — it names what the rebuild could not build,
  which is the actionable half ("pool dir '…' is not a directory"), and inventing a
  wrapper would bury it.

## Evidence

- `crates/host/src/lib.rs`:
  - `a_refused_undo_leaves_the_history_alone` — mixer, pool, track, clip, then a
    move; the pool directory is deleted and the undo is issued. It asserts the
    `Err` names the pool, that the clip is still at frame 4 800, that `can_undo` is
    still true and `can_redo` false, and — after the pool is put back — that the
    *next* undo reverts the move and leaves the clip itself on the track. On the
    unfixed code it fails at `!s.can_redo()`, with the moved entry sitting on the
    redo branch it was never applied to.
  - `a_refused_redo_keeps_the_redo_branch` — the mirror: the undo succeeds while
    the pool is there, the pool is deleted, the redo is issued. It asserts the
    `Err`, that the clip is still at frame 0, that `can_redo` is still true, and
    that a redo after the pool returns re-applies the move. On the unfixed code it
    fails at `can_redo()` — the branch's only entry is gone.
  - `undo_and_redo_revert_and_reapply_an_edit`,
    `undo_with_nothing_to_undo_is_a_noop`,
    `an_undo_that_removes_a_refused_edit_clears_the_report`,
    `last_seek_ignores_undo_and_redo` and the gesture tests are unchanged: a
    successful undo is still a rebuild, and the refusal rule only fires on the error
    path. 83 host tests green (81 before this change), 1 ignored.

## Alternatives considered

- **Snapshot both stacks before the mutation and restore them on `Err`** (the shape
  the report suggested). Correct, and rejected on cost alone: `history` is the whole
  session log and `redo` a list of gesture entries, so a refusal after a two-hour
  session deep-copies every command in it to learn that nothing changed. The copy
  this ships is bounded by one paste.
- **Build the candidate history in a local and assign it only after the rebuild
  succeeds** (the report's other suggestion). Rejected as the larger change: the
  rebuild reads `self.history` through `replay_to_kind` → `rebuild`/`rebuild_prefix`,
  so threading a candidate in means changing the replay contract for every caller
  (a seek included) to fix a defect that two restores close. Worth revisiting if
  undo ever stops being a rebuild at all — see the checkpointing below.
- **Re-apply the entry by calling `execute` again** on the refusal. Rejected: it
  re-runs the gesture through the value model, which can refuse it a second time for
  an unrelated reason, and it would journal the entry as a fresh edit.
- **Leave the stacks desynchronised and refuse to offer undo/redo when the last
  replay failed** (a latch: `undo_disabled`). Rejected: a latch outlives its subject.
  The refusal is a property of the *material on disk*, not of the history, and the
  history is what has to keep describing the session — the note's sibling argument
  against the journal latch applies verbatim here.
- **Swallow the error and keep the edit applied** (undo reports `Ok(false)` when the
  rebuild is refused). Rejected: the user would be told the edit was undone when the
  next render still has it, which is the same class of lie in the other direction.

## Consequences

- A refused undo or redo is a **no-op plus an error**: the arrangement, the graph,
  `can_undo`/`can_redo` and the next `save` all keep describing the session the user
  is actually looking at. The failure is now visible and harmless rather than silent
  and cumulative.
- An undo or redo issued while a take is in progress still finalizes the take on the
  way into the rebuild — that is `replay_to_kind`'s rule, shared with a seek and
  with the live load, and it happens before the replay that may then be refused. The
  take lands in the pool either way; only the edit is put back.
- `undo` clones one gesture entry per call. It is on the control path, next to a
  full session replay, so the clone does not register; nothing on the render path
  changed.
- Still open, and the same answer as the undo note's: a history long enough for the
  replay to dominate is the point at which a checkpointed history is the fix — not a
  different failure rule.

## Attribution

*Authored with Space Bunny · OpenCode, 2026-09-29.*
