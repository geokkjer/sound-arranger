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

**The first fix left the load path unwalked, so the defect survived it.** `rebuild` and
`rebuild_prefix` were converted; `HostSession::from_script` (`crates/host/src/lib.rs`) — the
loader `load_session` calls — was not. It issues each command with `execute`, which renders
nothing between the script's lines, so the unplaced triple

```text
mount clock_out
unmount clock_out
mount clock_out
```

is re-issued with an empty apply queue and the second mount is refused the same way. The
`bounce` that released the name on the live path is an **action**, so it is not in the
saved text: the file faithfully records a session the platform cannot open.
`load_session` returned `Err("plugin 'clock_out' is already mounted")` and the session was
lost. `apply_journal` — `load_session`'s second half, the autosave replayed on top — had
the same gap with a worse failure: a refused journal entry is **dropped and reported**, so
the load *succeeded* while silently losing the user's last edit. `save`'s round-trip check
(the parsed text must equal the history) does not catch either, because the text is
faithful — it is the *application* of the text that failed.

**The walk admitted frame-inverted documents, which orphaned a node and dropped a disposer.**
The walk tracked the lifecycle in *log* order while the apply queue drains in *frame* order,
so `Mount p@0, Unmount p@48000, Mount p@12000` was admitted and then applied as two
`apply_mount`s with no `apply_unmount` between: `node_of`/`disposers` overwrite the first
entry, its node stays in the graph, and its `Box<dyn FnOnce>` is dropped unrun. The engine
writes that log itself whenever a caller places the clock back before a scheduled frame
(`Engine::seek`), and `rebuild_prefix` manufactured one from a *legal* history (below).

**And `enter_walk` documented a safety property it did not have.** The doc claimed a walk
"is seeded from what the engine already holds, so it never loses the target's own state".
The seed was a `HashSet` snapshot taken at entry; a snapshot goes stale the moment the
target's own state moves, and a stale set is not a view of anything. A public method whose
doc overstates is worse than one that documents its footgun.

## Decision

**The one-instance-per-name rule is answered from the document's own lifecycle while a
document is being applied — and only then.** One mechanism, five call sites.

- `Engine` owns a private `Live` map and two entry points, `enter_walk` / `leave_walk`: a
  **document walk** is a caller re-issuing recorded state (a log, the host's command history, a
  session script) onto an engine. While a walk is in force, `holds_instance_of` — the
  one-instance question — reads the walk's `Live` **for the names the document has spoken
  for**, and falls through to the engine's own `disposers` + `scheduled` for every other name.
  `Engine::mount` takes the name, `Engine::schedule_unmount` gives it back, so
  `mount … unmount … mount` is a re-mount in a document exactly as it is on the live path
  (where the release happens at `apply_unmount`). Walks stack, and a nested walk starts from
  the document state it is nested in. **A walk is a view of the document, not a seed of the
  target** — that is the whole of the fix to the stale-snapshot claim above, and it is what
  makes the one-instance answer true in both directions: the document cannot forget a name it
  has released, and it cannot claim a name the target holds without releasing it first.
- **A frame-inverted document is refused, loudly, at the walk.** `Live` records, per name, the
  frame of the document's most recent lifecycle event; a mount or an unmount stamped *before*
  that frame returns `Err(Self::frame_inverted(name, previous, next))`, which names the
  plugin, both frames, and the consequence. The alternative — making the apply heal itself, by
  having `apply_mount` dispose whatever it finds under the name — was rejected: it would render
  something the log does not say, and it would hide a log-order error behind plausible audio on
  a path whose whole contract is that rendering is a pure function of the log. The refusal
  happens **before** the event is scheduled or logged, so a refused event leaves no trace, and
  it is a `Result` on every build — never a panic. `Engine::schedule_unmount` is now fallible
  for exactly this reason; off the walk it cannot fail, so the live path's behaviour is
  unchanged.
- `replay_from` refuses a non-fresh target **loudly**, before it walks: a plugin that is
  already applied (or still queued — nothing has rendered, so both are pre-existing state)
  means the log's mounts would be applied *over* a live instance, and `apply_mount`'s
  `node_of`/`disposers` insert would overwrite it — the first instance's node stays in the
  graph and its disposer is dropped with its entry. `replay_needs_fresh_engine` is that
  refusal. The walk is entered and left around the loop, so the log owns the lifecycle for the
  duration.
- `HostSession::rebuild`, `rebuild_prefix`, **`from_script`** and **`apply_journal`** enter a
  walk around their state walk, scoped with **no `?` between the enter and the leave**, so a
  refused state command cannot leave a rebuilt (or half-loaded) session walking.
  `apply_journal`'s leave is unconditional, because its loop already tolerates a refused
  entry. Nothing else changed on these paths: the same commands are issued, in the same order,
  with the same placements; only the owner of the lifecycle changed. The mixer is not
  special-cased, and neither is any other plugin.
- **`rebuild_prefix` no longer walks the clock backwards.** It places the clock at each
  `SetTempo`'s stated frame so the tempo segment sits where it belongs, and it did that
  unconditionally — so a tempo command whose placement is *already behind* the clock moved the
  clock back, and every `at_now` command after it was stamped before the commands before it.
  The live path does not do this: `process` renders up to a placement only when it is in the
  future, and `set_tempo` stamps at the current frame otherwise. `at_frame.unwrap_or(0).max(clock.frame())`
  is that rule in one expression, and without it a legal history (a tempo placed in the past, a
  plugin re-mounted after it) produced a document the new frame rule correctly refused.
- The replayed `Mount` records **both** halves of the apply queue's view, as the live `mount`
  does: `scheduled` and `scheduled_params`. Without the second, `ports_of`/`params_of` fall
  through to the nominal `port_table` while the mount is still queued, so a `Patch` to a port
  the queued instance would not have validated against the catalog and only failed later at
  `apply_patch`'s `debug_assert!`. A replayed `ScheduleUnmount` is deliberately **not** mirrored
  into either: that is the apply queue's own view, and it is drained at apply.
- The live path is unchanged: `Engine::mount` still refuses a second instance against
  `disposers`/`scheduled`, including inside the same-tick `mount → unmount → mount` window that
  has not applied yet.
- Tests, engine (`crates/engine/tests/spike_a.rs`, beside `remount_reproduces_identical_signal`):
  `replay_accepts_a_log_that_re_mounts_a_plugin`,
  `replay_onto_an_engine_that_already_has_plugins_is_refused`,
  `replay_refuses_a_log_whose_mounts_are_not_in_frame_order` (builds the inverted triple with
  the **live** API — `mount; render; schedule_unmount @96 000; render; seek(0); mount` — so the
  refusal is about the engine's rule and not about a hand-built fixture; then asserts the
  frame-*ordered* triple is still accepted and leaves exactly one node), and
  `a_walk_answers_for_the_documents_names_only` (pins the half of `enter_walk`'s contract that
  is enforced, so the doc cannot rot).
- Tests, host (`crates/host/src/lib.rs`): `a_session_that_re_mounts_a_plugin_still_exports_and_seeks`
  builds the triple twice — the bus, and a plugin that is not the bus so the clips keep playing
  across the re-mount — and then exports, warm-seeks (`rebuild_prefix`), full-replay-seeks
  (`rebuild(Some(frame))`) and renders. The remount test renders in the same call boundaries as
  the live run, because the mixer's unmount/remount changes the master width and such an event
  parks to a render-call boundary (the [control→render handoff
  note](../architecture/2026-08-27-control-render-handoff-parked-ops.md)).
  `a_saved_session_that_re_mounts_a_plugin_reopens` is the **round trip** the first fix did not
  make: build, `save`, `load_session` into a fresh session, then export, bounce, seek and save
  again (a second load has to work too). `a_journal_that_re_mounts_a_plugin_is_applied_not_dropped`
  writes an unplaced re-mount triple into a session directory's `journal.txt` and asserts
  `applied: 3, refused: 0` — the failure it guards against is a *silent* loss, so the assertion
  is on the recovery report, not on the absence of an error.
  `a_warm_seek_over_a_past_tempo_placement_still_works` is the `rebuild_prefix` regression, and
  it asserts the live path's lifecycle frames for the re-mounted plugin are non-decreasing —
  the invariant the frame rule rests on.

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
  non-fresh engine.** The first half is what shipped (`replay_from`); the walk answers per name
  instead, because the host's rebuild builds a *fresh* session every time and a walk is also a
  fine way to continue a session that already holds plugins — refusing it wholesale would be a
  rule about a case that cannot arise from the host. The per-name fall-through is what makes
  that safe: a walk that mounts a name the target holds is still refused.
- **Make the apply heal itself** — have `apply_mount` dispose whatever instance it finds under
  the name before applying, so a frame-inverted document can never orphan a node. Rejected: it
  would render something the log does not say, and a log-order error would surface as plausible
  audio instead of a message. The engine's contract is that rendering is a pure function of the
  log; a self-healing apply is a second, silent interpretation of it.
- **Sort the document by frame before walking it**, so an inverted log applies in a sensible
  order. Rejected for the same reason as above, and it is worse: it would change the log rather
  than refuse it, and the user would get audio from a document they did not write.
- **Keep `enter_walk` infallible and document the footgun only.** Rejected on the evidence: the
  doc's claim was not merely optimistic, it was checkable and *false* (`Live::of` snapshotted
  the target, and `holds_instance_of` consulted the snapshot instead of the engine). Making the
  structure a view rather than a seed is what makes the claim true, and the test
  `a_walk_answers_for_the_documents_names_only` is what keeps it true. A guard type is not
  available here — a guard would have to borrow the engine, which is the thing being mutated
  inside the walk — so the doc says plainly what the caller owes, and the enforcement lives
  where the knowledge is.
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
- The render path is untouched: the walk is control-side, entered and left per replay, per
  rebuild, per load or per journal.
- **A saved session that re-mounts a plugin reopens, and a journal that re-mounts one is
  applied rather than dropped.** Before this, `load_session` failed outright on such a file and
  `apply_journal` failed *quietly* on such a journal — the load reported success with the last
  edit missing. Both are the same walk, not a second mechanism.
- **A frame-inverted document is now refused, loudly, on every walk** — replay, load, journal,
  rebuild — with a message naming the plugin, both frames and the consequence. It is a `Result`
  on every build, so it is never a panic. The failure mode moves from "a valid log silently
  leaks a node and drops a disposer" to "an invalid log says so", which is the whole point of
  the invariant this note exists to keep.
- **`Engine::schedule_unmount` is fallible now.** It cannot fail off a walk, so the live path is
  unchanged; the signature changed because the walk needs it to speak. Two call sites outside
  this crate would need `.expect(...)` — there are none in the tree.
- **`rebuild_prefix` no longer moves the clock backwards**, so a tempo command whose placement is
  already behind the clock no longer stamps the rest of the rebuild before itself. This was
  invisible before only because nothing checked; the frame rule turned it into a loud refusal,
  and the `max` is the live path's own rule rather than a new one.
- **Stated obligations, not enforced ones.** A walk cannot tell which caller issued a command
  inside it, so `enter_walk`'s doc says that entering one asserts everything issued until the
  leave is one document, and names the two ways to break it (issuing state of your own; placing
  the clock backwards). `Engine::seek` carries the same statement, because `Clock` is a public
  field and there is nothing to enforce it with. The enforced half — a walk cannot mount over a
  name the target holds — is pinned by `a_walk_answers_for_the_documents_names_only`.
- The live `unmount`-before-apply edge the spike notes deferred ("the scheduler holds no cancel
  semantics yet") is still deferred — that is a live-path question, not a document one.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
