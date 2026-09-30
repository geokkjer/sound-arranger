# Agent Note: undo/redo over the session log

Status: implemented

## Problem

The timeline became an editor ([note](2026-09-10-timeline-clip-editing.md)) but the session log was
append-only: there was no way back from a move, trim or split, and the top bar's `⟲`/`⟳` buttons
were disabled chrome. A destructive-feeling editor without undo is not usable for real arranging.

## Decision

- **Undo/redo lives in the host, over the same history a seek already replays.** `HostSession` gains
  `redo: Vec<(usize, HostCommand)>`; `history: Vec<HostCommand>` ([note](../architecture/2026-09-09-host-transport-seam.md))
  was already the session's reconstruction log, so undo needs no second mechanism.
- **Only `Arrange` ops are undoable.** `undo` scans back for the last `Arrange` in `history`, removes
  it, pushes `(position, command)` onto `redo`, and rebuilds. A `Mount`/`Pool`/`SetTempo` is session
  *setup*, not an edit: undoing one would tear the graph down under every panel. `can_undo` is
  therefore "history contains an `Arrange`", not "history is non-empty".
- **A refused rebuild undoes the undo.** Both stacks describe the candidate session while it is
  being built, so the edit goes back where it came from when the replay is refused — a rebuild is
  fallible for reasons that have nothing to do with the edit (a pool directory that moved, a deleted
  clip). `Err` means "no edit was applied"
  ([note](../bug-fix/2026-09-29-a-refused-undo-changes-nothing.md)).
- **Redo re-inserts at the remembered index.** The op goes back at its original history position (not
  appended), so the reconstruction order — and every later command that depended on the frame
  arithmetic — is faithful. `redo.pop()` is LIFO, matching the undo order.
- **A new state command clears `redo`.** `process` pushes to `history` and clears `redo` together
  (`cmd.is_state()`): once you edit after undoing, the undone branch is unreachable. Actions
  (transport, seek) are not state and do not clear it.
- **Undo rebuilds to the *current* frame, not the edit's frame.** `replay_to(frame)` constructs a fresh
  `HostSession`, replays `history`, renders to the given frame, then restores the `playing` flag and
  the `redo` stack by moving the rebuilt session into `self`. The playhead therefore never jumps and
  playback is not interrupted — undo is an edit, not a time-travel.
- **The wire surface is one small struct.** `Snapshot` carries `can_undo`/`can_redo`; the bridge
  publishes them as `TransportState.edit` (`EditState { can_undo, can_redo }`) for the 25 Hz poll and
  as `ScriptOutcome.edit` for the edit itself, so the buttons are exact on both paths (an edit's own
  outcome tells the UI immediately; undo/redo refresh with one poll). The text grammar gains `undo`
  and `redo` lines, so the one-shot script path and the inert-command listing stay complete.
- **The shell owns the gesture.** `editor.undoEdit`/`redoEdit` invoke `edit_undo`/`edit_redo` and
  re-poll; `bridgeState.canUndo`/`canRedo` gate the top bar's `⟲`/`⟳`; `Ctrl/Cmd+Z` and
  `Ctrl/Cmd+Shift+Z` are bound at the window, skipping events that originate in an input or textarea
  so the host-script box keeps its own text undo.

## Alternatives considered

- **An inverse-op journal** (every command ships a compensating command, per the Cordis/`∂Γ` lineage in
  `RESEARCH.md`). Rejected for now: it doubles the op vocabulary and every new op must also ship a
  correct inverse; replaying a log we already keep is smaller and cannot drift from the forward path.
- **Snapshot-based undo** (clone the whole `HostSession` before each edit, pop to restore). Rejected:
  the session owns the engine, the mixer, the arrangement and the pool resolver — cloning it per
  gesture is far more expensive than replay, and the replay path already exists for seek.
- **Making every state command undoable** (mounts, pool, tempo). Rejected: setup is not an edit and
  undoing it would unmount plugins under the UI; the history exists to reconstruct the session, not to
  offer time-travel over the whole configuration.
- **Undoing to the edit's frame** (time-travel semantics). Rejected: the playhead would jump backwards
  while playing. The user's mental model is "the clip moved", not "we went back in time".
- **A frontend-side undo stack** (keep the op lines in `editor.ts` and invert them there). Rejected:
  it duplicates the log, and the shell would have to invert ops the engine validates — the host
  already owns the authoritative history.
- **Per-`pointermove` history entries.** Already rejected in the clip-editing note; undo inherits that
  decision, so one gesture is one undo step.

## Consequences

- Undo/redo is a **rebuild**: cost is a replay plus a render to the current frame. That is the same
  cost as a seek, so it is acceptable at the current scale and bounded by the session log's length;
  if a session ever grows to where this hurts, a checkpointed history is the fix, not a different
  undo model.
- Mounts, the pool, tempo and mixer settings are **not** undoable; the buttons only light up for
  arrangement edits. This is deliberate and visible (`can_undo` is false on a freshly loaded script).
- The `redo` stack is unbounded and cleared by the next edit — standard linear-history behaviour.
- Playback survives an undo: the rebuild restores the `playing` flag, and only `TransportStop` and
  `Load` drain the ring ([note](../architecture/2026-09-09-host-live-runtime-actor-pump.md)), so an edit while playing
  is audible on the next rendered chunk.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-10.
