# Agent Note: A refused live load keeps the session it refused to replace

Status: implemented

## Problem

`HostHandle::load` is the live bridge's "run this script" gesture: the actor builds
a **fresh** session, applies the commands, adopts it and returns the outcome
([shell live bridge](../architecture/2026-09-09-shell-live-bridge.md)). Building
fresh is what makes re-running a script idempotent — no double-mount.

The **failure** path of that gesture was not idempotent, it was destructive. When
`HostSession::from_script` returned `Err` (a typo, a missing pool, a command the
engine refuses), the actor installed a `HostSession::new()` — an empty session — in
place of the one it was driving, re-anchored the pump, paused the device and drained
the ring, then returned the error. So the caller learned its script was refused
while the session it had been editing, playing and recording against was already
gone: the arrangement, the pool binding, the history (so nothing left to undo) and
any edit not yet saved. The shell then published the empty session forever after,
because that is what the actor was now holding. A gesture that is *meant* to be
re-runnable destroyed the session on a typo.

The command form of the same operation already had the right rule one function
away: `load_session` builds the candidate, replays the journal onto it, and only then
writes `*self = loaded` as its last statement, so a refused load leaves `self` alone.
The two spellings of "open a session" disagreed about what a failure means, and the
live one — the one with a transport, a pool and a user's unsaved work behind it —
was the wrong one.

The same arm had the mirror defect on the success path: adopting the fresh session
dropped a take in progress on the floor. The `Capture` was still open, its WAV had
no finalized header, and the audio was gone. `replay_to_kind` (a seek, an undo) had
already settled this — stop the take first so it lands in the pool and is reported —
and a load is exactly as much a session replacement as a seek is.

## Decision

**A load builds its candidate first and adopts it only once it built whole; a
refused load returns the error and leaves the live session — transport, arrangement,
pool binding, history, meters and the published snapshot — exactly as it was.**

- `run`'s `Request::Load` arm matches on `HostSession::from_script`: on `Ok` it
  adopts (`session = fresh; anchor = None;`) and only then pauses the device and
  drains the ring, because those belong to the session being replaced; on `Err` it
  sends the `Err` and republishes the session that is already there. The pump's
  anchor, the transport and the output ring are untouched, so a refused load is
  indistinguishable from a refused `HostCommand`.
- The two paths can no longer drift, because they are now the same rule in the same
  shape as `load_session`: build, then adopt.
- **A take in progress is stopped before the adoption**, the way `replay_to_kind`
  does it, and the report is handed to the session that survives
  (`HostSession::status_take`) so the outcome the shell reads names the take that
  just landed in the pool. The report is taken from the session rather than from
  `stop_recording`'s result, because `stop_recording` keeps it even when finalizing
  complained — a take that landed with a complaint is still placeable.

## Evidence

- `crates/host/src/live.rs`:
  - `a_refused_load_keeps_the_live_session` — load a mixer, play, then load a script
    that parses but is refused (`mount mixer channels=2.5`, the mixer's own
    positive-whole-number rule). It asserts the `Err` names the rule, that
    `outcome()` still reports the 2-channel mixer, that the transport is still
    running, and that a good load after it still replaces the session as before. On
    the unfixed code it fails with `mixer_channels: None` — the empty replacement.
  - `live_host_loads_a_script_and_reloads_cleanly` still holds, unchanged: a
    successful load is still reset-and-apply, and the re-load still does not
    double-mount. The neighbouring live tests
    (`live_host_executes_commands_and_refusals_survive`,
    `live_host_plays_advances_and_stops`, `the_snapshot_publishes_the_midi_status`)
    are unchanged. 68 host tests green.
- The take-in-progress half has no automated test: a take in a live actor needs an
  input device (`record <id>` opens one; `start_recording` is not reachable from a
  `HostCommand`), so the live path can only be exercised with hardware. The rule and
  the code are the same as the seek's, which is covered host-side.

## Alternatives considered

- **Publish an empty session and let the shell re-load** (leave the behaviour, fix
  the shell). Rejected: the actor's whole value is that the session outlives the
  call. A shell that has to notice the swap and re-issue the script is the stateless
  bridge this note's sibling retired, with the transport stranded in between.
- **Keep the old session, but return a synthesized empty outcome alongside the
  error** so the caller learns what it did not get. Rejected: an outcome describes
  *a* session; reporting one for a session that was never adopted is how a caller
  ends up drawing the wrong meters. One answer per request — the error, or the
  outcome of the session now installed.
- **Adopt a partially applied session** (keep whatever the script managed to mount
  before the refusal, like `apply_journal` tolerates a bad journal entry). Rejected:
  it invents a session the user never wrote, with a pool and a plugin set that no
  script describes, and it is the opposite of the determinism `from_script`'s
  document walk exists to provide. A refused script is refused whole.
- **Make the shell parse the script before issuing the load**, so the actor never
  sees a refused one. Rejected as a fix (it does not cover a command the engine
  refuses rather than the parser, e.g. a missing pool or a bad mixer channel count —
  precisely the reported triggers) and it duplicates the parse in every caller.
- **Leave the take in progress dropped on the success path** (the report's core fix
  alone). Not a live option: the capture's WAV would keep an unfinalized header and
  the audio would vanish with the session, so the change would have traded a
  recoverable error for an unrecoverable one on the very gesture it fixed.

## Consequences

- A refused `load` is now a **no-op plus an error**: the session the shell is driving
  keeps its arrangement, its pool, its transport, its undo history and its meters,
  and the published snapshot is unchanged. A typo in a live script costs the typo.
- `HostHandle::load`'s contract is now stated honestly: `Err` means "nothing was
  loaded", never "something was unloaded". The
  [shell live bridge note](../architecture/2026-09-09-shell-live-bridge.md) says so
  on its `run_host_script` bullet.
- A `load` during a take finalizes that take (WAV + peaks in the pool) and reports
  it in the outcome, instead of dropping the capture. This is the same rule a seek
  and an undo already follow; the difference is that a load's *report* survives into
  the new session instead of being overwritten by the rebuilt one's.
- No cost on the render path: the arm runs on the control path, once per load.
- Still open: the shells' load gesture has no confirmation for "refused, nothing
  changed" beyond the error it already shows, and a headless test for the take
  branch would need a `HostCommand` that starts a take from a ring
  (`start_recording` is the seam and is not exposed as a command).

*Authored with Space Bunny · OpenCode, 2026-09-29.*
