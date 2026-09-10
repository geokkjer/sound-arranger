# Agent Note: incremental arrange ops over the bridge

Status: implemented

## Problem

The bridge could **load a whole script** (`run_host_script` = reset-and-apply) and drive transport, but
there was no way to apply **one** arrangement op to the live session. A UI therefore could not edit at
all: every "move this clip" would have meant re-sending the entire script, which resets the session and
replays it — wrong for a session the user is working in, and it would fight anything the transport is
doing.

## Decision

Add the incremental edit path, reusing the **text-format op grammar** as the op vocabulary:

- **`host::parse_arrange_line(&str) -> Result<(ArrangeOp, Option<u64>), String>`** — parses a single
  `arrange …` line (the `arrange` keyword optional, a trailing `@frame` allowed), delegating to the
  same `parse_arrange` the script parser uses. One op grammar, so there is no second schema to drift
  against (the Tauri review's F3).
- **`HostHandle::outcome() -> Result<HostOutcome, String>`** — a read-only outcome query on the actor
  (arrangement + diagnostics), so the bridge can return the refreshed value after an edit.
- **The bridge's `arrange(line)` command** (`apply_arrange`): parse → `HostCommand::Arrange { op,
  at_frame }` → `outcome()` → the serializable `ScriptOutcome`. A refused op is an `Err` and changes
  nothing (the engine's validate-first rule), so the UI can surface it.
- `to_wire` now takes `bounce_written: bool` (computed by the caller) instead of the command list, so
  both the load and the edit paths share it.

## Alternatives considered

- **A serde `ArrangeOp` DTO** (derive/wire a second representation of the op enum). Rejected: it is a
  *second grammar over the same vocabulary*, exactly the drift surface the review flagged. The text
  format is the wire schema; parse one line of it.
- **Send a whole script per edit** (re-run `run_host_script`). Rejected: reset-and-apply discards the
  live session and replays everything for a single move — and it would restart the graph mid-playback.
- **A bespoke Tauri command per op** (`move_clip`, `trim`, …). Rejected: same duplication, more
  surface, and each would need its own validation story.
- **Batch several ops per call.** Not needed yet: the interaction model is one op per gesture (the
  drag previews locally and commits once on release), so the round-trip is per gesture, not per frame.

## Consequences

- The UI can now edit: move, trim, razor-split, duplicate, delete, gain/fade — anything in the op
  grammar — one gesture at a time.
- Edits arrive as `HostCommand::Arrange`, which is already in the session's **state-command history**,
  so a seek-rebuild preserves them (they are part of the reconstructed session, not lost).
- Each edit is a parse → execute → outcome round-trip. Fine per gesture; if a future interaction needs
  per-frame edits, that wants a batched/streamed path.
- **Undo is still absent** — the log is append-only and the host has no inverse-op or snapshot
  history. That is the next piece.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-10.
