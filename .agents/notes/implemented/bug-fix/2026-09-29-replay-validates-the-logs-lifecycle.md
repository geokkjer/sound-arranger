# Agent Note: Replay validates the log's lifecycle, not the engine's scheduling state

Status: implemented

## Problem

`Engine::replay_from` walked the session log and asked `validate_mount` "is this name already
mounted?" for every `Mount` it replayed. `validate_mount` answered from the **engine's** live
state — `disposers` (applied mounts) and `scheduled` (mounts queued but not yet applied). A
replay applies nothing (nothing renders while the log is being replayed), so `scheduled`
accumulated every mounted name for the whole walk and never lost one. A log of the shape

```text
Mount p … ScheduleUnmount p … Mount p
```

failed at the third event with `plugin 'p' is already mounted (one instance per name in spike
A.5)` and abandoned the replay — including the events before it.

That shape is not exotic; it is what the live engine produces. `unmount` schedules an `Unmount`,
and the name is released when the render loop **applies** it (`apply_unmount`), so mounting the
same name again after that unmount applied succeeds — the engine's own test
`remount_reproduces_identical_signal` builds exactly such a log (mount chain → render → unmount
chain → render → re-mount chain). The log is the document (invariant 2), and the document the
engine writes could not be read back by the engine.

**The same defect was shipped in the host, one module over.** The host never calls
`replay_from`: its re-apply paths — `HostSession::rebuild` (`crates/host/src/lib.rs`) and
`rebuild_prefix`, reached by `replay_to_kind` (seek, `undo`, `redo`) and by `export` through
`export_detached` — re-issue `self.history` as `HostCommand`s. With their placements stripped
(`HostCommand::at_now`) or simply absent, `process` renders nothing between the commands, so
the apply queue never drained and the second `Mount` was refused the same way. `parse_script`
accepts `@<n>` on `mount`/`unmount` and `from_script` renders between them, so a session
script like

```text
mount mixer channels=2.0 @0
unmount mixer @96000
mount mixer channels=2.0 @192000
```

was **accepted and recorded** by the live path and then could not be exported or seeked:
`Err("plugin 'mixer' is already mounted (one instance per name in spike A.5)")` out of `export`
and out of every seek. The same holds for an unplaced triple (what an interactive `execute`
writes), which no path renders between.

## Decision

**The one-instance-per-name rule is answered from the document's own lifecycle while a
document is being applied — and only then.** One mechanism, three call sites.

- `Engine` owns a private `Live` set and two entry points, `enter_walk` / `leave_walk`: a
  **document walk** is a caller re-issuing recorded state (a log, the host's command history)
  onto an engine. While a walk is in force, `holds_instance_of` — the one-instance question —
  reads the walk's `Live` set; the live path still reads `disposers` + `scheduled`. `Engine::mount`
  takes the name, `Engine::schedule_unmount` gives it back, so `mount … unmount … mount` is a
  re-mount in a document exactly as it is on the live path (where the release happens at
  `apply_unmount`). A walk is seeded from what the engine already holds, so a walk can never
  forget a name the target has in force, and walks stack.
- `replay_from` refuses a non-fresh target **loudly**, before it walks: a plugin that is
  already applied (or still queued — nothing has rendered, so both are pre-existing state)
  means the log's mounts would be applied *over* a live instance, and `apply_mount`'s
  `node_of`/`disposers` insert would overwrite it — the first instance's node stays in the
  graph and its disposer is dropped with its entry. `replay_needs_fresh_engine` is that
  refusal. The walk is entered and left around the loop, so the log owns the lifecycle for the
  duration.
- `HostSession::rebuild` and `rebuild_prefix` enter a walk around their state walk, scoped with
  **no `?` between the enter and the leave**, so a refused state command cannot leave a
  rebuilt session walking. Nothing else changed: the same commands are issued, in the same
  order, with the same placements; only the owner of the lifecycle changed. The mixer is not
  special-cased, and neither is any other plugin.
- `validate_mount` splits in two. The one-instance guard stays in it (that guard *is* about
  this engine's scheduling state); everything else — mount params finite, plugin known, declared
  services provided — moves to `validate_mount_declaration`, which the walk calls.
  `Self::already_mounted` holds the refusal string, so the paths cannot drift apart.
- The replayed `Mount` records **both** halves of the apply queue's view, as the live `mount`
  does: `scheduled` and `scheduled_params`. Without the second, `ports_of`/`params_of` fall
  through to the nominal `port_table` while the mount is still queued, so a `Patch` to a port
  the queued instance would not have validated against the catalog and only failed later at
  `apply_patch`'s `debug_assert!`. A replayed `ScheduleUnmount` is deliberately **not** mirrored
  into either: that is the apply queue's own view, and it is drained at apply.
- The live path is unchanged: `Engine::mount` still refuses a second instance against
  `disposers`/`scheduled`, including inside the same-tick `mount → unmount → mount` window that
  has not applied yet.
- Tests: `replay_accepts_a_log_that_re_mounts_a_plugin` and
  `replay_onto_an_engine_that_already_has_plugins_is_refused`
  (`crates/engine/tests/spike_a.rs`, beside `remount_reproduces_identical_signal`), and
  `a_session_that_re_mounts_a_plugin_still_exports_and_seeks` (`crates/host/src/lib.rs`), which
  builds the triple twice — the bus, and a plugin that is not the bus so the clips keep playing
  across the re-mount — and then exports, warm-seeks (`rebuild_prefix`), full-replay-seeks
  (`rebuild(Some(frame))`) and renders. The remount test renders in the same call boundaries as
  the live run, because the mixer's unmount/remount changes the master width and such an event
  parks to a render-call boundary (the [control→render handoff
  note](../architecture/2026-08-27-control-render-handoff-parked-ops.md)).

## Alternatives considered

- **Clear `self.scheduled` for a replayed `ScheduleUnmount`.** One line, and it makes the walk's
  bookkeeping mirror `apply_unmount`. Rejected: it corrupts the apply queue's own view.
  `scheduled` answers "is this name queued-but-not-applied?", and a replayed mount with a *later*
  unmount would look absent to a patch that targets it, while a mount that has not applied yet
  must still be answerable. One set, two different questions — the lifecycle deserves its own.
- **Apply the events as they are replayed (replay becomes a rebuild).** Rejected: replay's
  contract is that nothing is applied eagerly, so the render loop reproduces the timeline
  exactly. Applying during the walk would break byte-identical replay and put graph work on the
  replay path. (The host's rebuild does render, between *placed* commands — which is why the
  defect needed fixing at all.)
- **Flush the apply queue after each state command in the host rebuild** (`flush_scheduled`),
  which drains `scheduled` and so admits the second mount. Rejected: it makes the rebuild
  *eager*, which is the same trade the note above rejects for replay; it runs `apply` (and a
  bus-width change, and the arrangement reconcile) at a point the seek machinery deliberately
  chose not to; and it fixes the symptom rather than naming the owner of the lifecycle.
- **Special-case the mixer in the host** (the mixer is *the* bus, so re-mount it specially).
  Rejected: the rule is the engine's, the failure is not the mixer's, and `clock_out`/`master`
  fail the same way. A second instance is still refused when the document really does mount a
  name twice.
- **Drop the one-instance rule in a walk and let a name mount twice.** Rejected: the rule is
  real — the plugin's name is its `node_of` key, so a second live instance overwrites the first's
  node and disposer. A document that does it without an unmount between is genuinely invalid and
  must stay a loud `Err`.
- **Refuse a replay whose target has *any* pre-existing state, or refuse a walk that starts on a
  non-fresh engine.** The first half is what shipped (`replay_from`); the walk seeds itself from
  the engine instead, because the host's rebuild builds a *fresh* session every time and a walk
  is also a fine way to continue a session that already holds plugins — refusing it would be a
  rule about a case that cannot arise from the host.
- **Widen the rule to instance suffixes (`name#2`).** Rejected as a larger decision than the
  defect: the engine never writes such a log, and multi-instance naming is the seam the
  [recorder/mixer note](../architecture/2026-08-18-p1-2-recorder-and-adaptable-mixer.md) already
  parked.

## Consequences

- Any log the engine writes now replays — the invariant the [patch-bay
  note](../architecture/2026-08-17-patch-bay-typed-signal-streams.md) states for patching
  (*replay reproduces byte-identical output*) was half-true until remount was exercised.
- A session that unmounts and re-mounts a plugin can be exported and seeked again, which it
  could not before: the re-apply paths are the ones the platform's own loader (`parse_script` +
  `HostSession::from_script`) writes into.
- `replay_from` refuses a dirty target with a loud `Err` naming the plugin, so a replay onto a
  used engine is a message rather than an orphaned node and a dropped disposer.
- A log that mounts a name twice with no unmount between is still refused, with the identical
  message. The failure mode moves from "valid logs refused" to "invalid logs refused", which is
  where it belongs.
- The **live** same-tick window is unchanged, so a script whose unmount carries no placement
  (`mount mixer` / `unmount mixer` / `mount mixer`, with nothing rendering between the lines) is
  still refused as it is typed: a re-mount needs the unmount to have *applied*, which needs a
  render or a placed command. A placed triple renders between the commands, an unplaced one
  works as soon as anything renders — and either way the history it leaves behind rebuilds.
- The render path is untouched: the walk is control-side, entered and left per replay or per
  rebuild.
- **Known and still open: log order vs frame order.** A walk tracks the lifecycle in *log*
  order, while the scheduler applies in *frame* order. A document that reads
  `Mount p@0, ScheduleUnmount p@100_000, Mount p@0` is therefore admitted, and at apply both
  mounts pop before the unmount, so `apply_mount` runs twice for one name — the first instance's
  node is never removed and its disposer is dropped. No in-tree caller produces it (it takes a
  caller abusing `Engine::seek` with a past-frame `schedule_unmount`, which validates neither),
  and the live path admits the same document today, so the walk neither causes nor worsens it.
  The honest fix is to key the walk on applied frame order (or to refuse a `ScheduleUnmount`
  whose frame precedes a live mount); it is not attempted here because it changes what a replay
  *accepts* on a path with no caller, and a wrong guess there is worse than the narrow hole.
- The live `unmount`-before-apply edge the spike notes deferred ("the scheduler holds no cancel
  semantics yet") is still deferred — that is a live-path question, not a document one.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
