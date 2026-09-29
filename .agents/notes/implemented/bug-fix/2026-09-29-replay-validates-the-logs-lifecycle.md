# Agent Note: Replay validates the log's lifecycle, not the engine's scheduling state

Status: implemented

## Problem

`Engine::replay_from` walked the session log and called `validate_mount` for every `Mount` it
replayed. `validate_mount` answers "one instance per name" from the **engine's** live state —
`disposers` (applied mounts) and `scheduled` (mounts queued but not yet applied). A replay applies
nothing (nothing renders while the log is being replayed), so `scheduled` accumulated every mounted
name for the whole walk and never lost one. A log of the shape

```text
Mount p … ScheduleUnmount p … Mount p
```

failed at the third event with `plugin 'p' is already mounted (one instance per name in spike A.5)`
and abandoned the replay — including the events before it. The session did not load at all.

That shape is not exotic; it is what the live engine produces. `unmount` schedules an `Unmount`, and
the name is released when the render loop **applies** it (`apply_unmount`), so mounting the same name
again after that unmount applied succeeds — the engine's own test `remount_reproduces_identical_signal`
builds exactly such a log (mount chain → render → unmount chain → render → re-mount chain). The log
is the document (invariant 2), and the document the engine writes could not be read back by the
engine.

Nothing tripped over it because no non-test caller of `replay_from` exists yet — the shipped host's
load/seek path re-issues `HostCommand`s instead — and no test replayed a log containing an unmount
followed by a re-mount.

## Decision

**The one-instance-per-name rule is answered from the log's own lifecycle while replaying.**

- `replay_from` keeps a replay-local `live: HashSet<&str>`: insert on `Event::Mount`, **remove on
  `Event::ScheduleUnmount`**. A `Mount` whose name is still `live` is refused with the same message
  the live engine speaks.
- `validate_mount` splits in two. The one-instance guard stays in it (that guard *is* about this
  engine's scheduling state); everything else — mount params finite, plugin known, declared services
  provided — moves to `validate_mount_declaration`, which replay calls. `Self::already_mounted` holds
  the refusal string, so the two paths cannot drift apart.
- `self.scheduled` is still updated during a replay (insert on a replayed `Mount`): `validate_patch`,
  `set_param` and `ports_of`/`params_of` ask it whether a name is queued-but-not-applied, and a
  replayed patch may legitimately target a mount that has not applied yet. A replayed
  `ScheduleUnmount` is deliberately **not** mirrored into it — that set is the apply queue's view, and
  whether the name is still live is now the log's question, not the queue's.
- The live path is unchanged: `Engine::mount` still refuses a second instance against
  `disposers`/`scheduled`, including inside the same-tick `mount → unmount → mount` window that has
  not applied yet.
- Regression test `replay_accepts_a_log_that_re_mounts_a_plugin` (`crates/engine/tests/spike_a.rs`,
  beside `remount_reproduces_identical_signal`): it renders the remount log live, replays that log
  onto a fresh engine, and asserts the audio is byte-identical. It renders in the same call boundaries
  as the live run, because the mixer's unmount/remount changes the master width and such an event
  parks to a render-call boundary (the [control→render handoff
  note](../architecture/2026-08-27-control-render-handoff-parked-ops.md)).

## Alternatives considered

- **Clear `self.scheduled` for a replayed `ScheduleUnmount`.** One line, and it makes the walk's
  bookkeeping mirror `apply_unmount`. Rejected: it corrupts the apply queue's own view. `scheduled`
  answers "is this name queued-but-not-applied?", and a replayed mount with a *later* unmount would
  look absent to a patch that targets it, while a mount that has not applied yet must still be
  answerable. One set, two different questions — the lifecycle deserves its own.
- **Apply the events as they are replayed (replay becomes a rebuild).** Rejected: replay's contract is
  that nothing is applied eagerly, so the render loop reproduces the timeline exactly. Applying
  during the walk would break byte-identical replay and put graph work on the replay path.
- **Drop the one-instance rule in replay and let a name mount twice.** Rejected: the rule is real —
  the plugin's name is its `node_of` key, so a second live instance overwrites the first's node and
  disposer. A log that does it is genuinely invalid and must stay a loud `Err`.
- **Widen the rule to instance suffixes (`name#2`).** Rejected as a larger decision than the defect:
  the engine never writes such a log, and multi-instance naming is the seam the
  [recorder/mixer note](../architecture/2026-08-18-p1-2-recorder-and-adaptable-mixer.md) already
  parked.

## Consequences

- Any log the engine writes now replays — the invariant the [patch-bay
  note](../architecture/2026-08-17-patch-bay-typed-signal-streams.md) states for patching (*replay
  reproduces byte-identical output*) was half-true until remount was exercised.
- `replay_from` still requires a **fresh** engine, as its doc says: the replay-local set answers the
  log's lifecycle, not the target engine's, so a name already mounted on the target is not consulted.
- A log that mounts a name twice with no unmount between is still refused, with the identical message.
  The failure mode moves from "valid logs refused" to "invalid logs refused", which is where it
  belongs.
- The render path is untouched: the new set is control-side, built once per replay.
- The live `unmount`-before-apply edge the spike notes deferred ("the scheduler holds no cancel
  semantics yet") is still deferred — that is a live-path question, not a replay one.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
