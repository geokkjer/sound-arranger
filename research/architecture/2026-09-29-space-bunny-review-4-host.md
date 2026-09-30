# Space-bunny external review — host crate

> **Disposition.** External deep review, 2026-09-29, against `49e8774` (clean tree). Reviewer:
> Space Bunny (`opencode/space-bunny-free`) driven through OpenCode by DeepSeek Harness; the
> supervisor (DeepSeek-V4.1-Flash) wrote the scope, then handed every CRITICAL/MAJOR claim to an
> **independent adversarial verifier** that had to falsify it before it counted.
>
> This file is the **host crate** scope, verbatim: the reviewer's report, then its verifier's
> verdicts. What was done about each finding — fixed, deferred or refuted — is in
> [the index](2026-09-29-space-bunny-review.md). Findings are the reviewer's, not the
> supervisor's: read the verdicts before acting on any single claim.

---

## 1. Reviewer's report (verbatim)

## host crate review

### MAJOR

#### 1. `wire_pending` resolves the mixer as `graph.out_node`, so any `play` in a `master` chain fails at render time — after the play is logged and committed

- **Severity** MAJOR
- **Location** `crates/host/src/lib.rs:1724-1741` (and the sibling lookup in the `Play` arm at `lib.rs:1486-1490`)
- **Trigger** `mount mixer channels=2 @0` + `mount master @0` + `patch mixer.audio master.audio @0` + `play /tmp/clip.wav ch0 @0` + `bounce N /tmp/out.wav`.
- **Wrong behaviour** The bounce fails with `player→mixer cord: no port 'ch0' on node 1`, the `MediaPlay` op is already in the session log, `media_commands` already counted, and the history already committed. The `PlaybackNode` is left in the graph permanently unconnected. A second trigger — `play` *after* the chain is already materialised — fails differently (`connect: patch cords must go forward`), because `insert_before(out_node)` puts the player *after* the mixer.
- **Evidence** `lib.rs:1724-1741`:

```rust
    fn wire_pending(&mut self) -> Result<(), String> {
        if self.pending_cords.is_empty() {
            return Ok(());
        }
        self.engine.flush_scheduled();
        let mixer = self
            .engine
            .graph
            .out_node
            .ok_or("play requires the mixer to be mounted (mount mixer first)")?;
        let pending = std::mem::take(&mut self.pending_cords);
        for (player, channel) in pending {
            self.engine
                .graph
                .connect(player, "audio", mixer, &format!("ch{channel}"))
                .map_err(|e| format!("player→mixer cord: {e}"))?;
        }
        Ok(())
    }
```

  `out_node` is the **bus owner**, not the mixer: `MasterPlugin::apply` calls `api.graph.set_out(node)` (`crates/engine/src/plugins/master.rs:514`), and `MASTER_PORTS` (`master.rs:48-61`) declares only `audio` in/out — no `ch0`. `wire_arranger` in the same file does this correctly and says why, at `lib.rs:1778-1792`: *"resolve it by **name**, not by `graph.out_node` (the bus points at whatever audio node was mounted last…)"* — that fix was applied to `wire_arranger` only.
- **Fix** Resolve the mixer by name in both `wire_pending` and the `Play` arm's pivot choice: `self.engine.flush_scheduled(); let mixer = self.engine.node_of("mixer").ok_or("play requires the mixer to be mounted (mount mixer first)")?;` and `insert_before(mixer, …)`.
- **Verification** Read `wire_arranger` (uses `node_of("mixer")`), `MasterPlugin::apply` (calls `set_out`), `MASTER_PORTS` (no `ch0`), `Graph::connect` (`port_index` → `None` → error) and `Graph::insert_before` (inserts at the master's index, i.e. after the mixer). Grepped the whole host crate: no test mounts `master` together with `play` — `the_master_chain_sits_on_the_bus_and_bounces_aligned` (`lib.rs:5510`) drives the arranger path, and `play_after_a_render_does_not_break_the_session` has no `master`. Not covered.

#### 2. An unparseable journal line makes the whole session unopenable; `journal_append` writes unvalidated text

- **Severity** MAJOR
- **Location** `crates/host/src/lib.rs:1310` (fatal `?`), `lib.rs:1332-1342` (unchecked write), `lib.rs:1272` (propagated to `load_session`)
- **Trigger** `load` a clean session (so `session_dir` is set), then execute a `Play`/`Splice` whose path contains a space or a `#` — which the host accepts (`resolve_clip`/`FilePlayer::start` handle any path). The next `load` of that directory fails and the session cannot be opened until the user hand-deletes `journal.txt`.
- **Wrong behaviour** `save` runs a full round-trip self-check before writing (`lib.rs:1197-1221`); `journal_append` has no equivalent check, so it writes a line that `parse_script` rejects. `apply_journal` then propagates the parse error out of `load_session`, contrary to its own doc comment two lines below the `?` (*"A journal entry the session refuses … is **dropped and reported**, never fatal"*). The save path already makes a whitespace path unsaveable, so the journal is the one remaining way the material lands — and it destroys loadability.
- **Evidence** `lib.rs:1307-1310`:

```rust
        let commands = parse_script(&format!("host v1\n{}\n", lines.join("\n")))?;
```

  `lib.rs:1332-1342`:

```rust
        let mut text = String::new();
        // The journal records *edits*, with the paths the session actually used. A
        // command with no text form cannot be in the history (a save would have
        // refused it), so a failure here means nothing to write.
        if write_entry(&mut text, entry, None).is_err() || text.is_empty() {
            return;
        }
```

  `format_command` for `Play` writes the path raw, so a spaced path becomes `play /tmp/a b.wav ch0` — four words against `exact(&words, 3, …)`. The same hole exists for `SetSourceTempo { source }` (`lib.rs:1545-1558` — no `valid_name` check, unlike `SourceAdd` at `lib.rs:1610-1615` which *does* call `media::valid_name`) and for `SourceAdd`'s `matcher` (only `is_empty()` is checked, `lib.rs:1621`).
- **Fix** (a) In `apply_journal`, drop-and-report an unparseable line group rather than `?`-ing, exactly as refused commands are handled. (b) Better: give `journal_append` the same guard `save` has — run the rendered entry through `parse_script` and set `journal_error` (skipping the write) if it does not round-trip. (c) Validate `source` with `media::valid_name` in the `SetSourceTempo` arm and `matcher` in the `SourceAdd` arm, for symmetry with `name`.
- **Verification** Read `write_entry`/`format_command` (`lib.rs:2814-2837`, `Play` arm ~`lib.rs:2935`), `apply_journal` (`1280-1328`), `load_session` (`1262-1278`), `execute` (`commit_state` → `journal_append`). `a_stale_journal_is_dropped_not_fatal` (`lib.rs:6906`) covers *refused* entries, and `a_save_that_cannot_round_trip_is_refused` (`lib.rs:6947`) covers the save path only. Not covered.

#### 3. A failed `undo`/`redo` mutates `history`/`redo` *before* the fallible rebuild, desynchronising them from the applied state

- **Severity** MAJOR
- **Location** `crates/host/src/lib.rs:2621-2634` (`undo`), `lib.rs:2636-2646` (`redo`), `lib.rs:2361` (the contract they break)
- **Trigger** Any session whose history fails to re-apply — the ordinary case being a deleted or moved pool directory, or a deleted `play` clip. Load a session, delete the pool, press Undo.
- **Wrong behaviour** `undo` returns `Err`, but the entry is already gone from `history` and already pushed onto `redo`, while `*self` was never replaced. The arrangement value and the graph still contain the edit. The next `save` writes a script **without** it, so reopening the session silently loses the change; `can_undo`/`can_redo` now describe a state that does not exist, and a second undo removes a different entry. `redo` has the mirror failure: it pops the redo stack and inserts into history before replaying, so a failure destroys the redo entry outright.
- **Evidence** `lib.rs:2630-2634`:

```rust
        let undone = self.history.remove(pos);
        self.redo.push((pos, undone));
        self.replay_to_kind(self.engine.clock.frame(), false)?;
        Ok(true)
```

  `lib.rs:2642-2645`:

```rust
        let at = pos.min(self.history.len());
        self.history.insert(at, cmd);
        self.replay_to_kind(self.engine.clock.frame(), false)?;
```

  `replay_to_kind` → `rebuild(Some(frame))` → `rebuilt.process(cmd)?` (`lib.rs:2460-2477`), and `process` for `Pool` reaches `media::Pool::open` via `set_pool`, so a deleted pool is a plain `Err`. The contract is stated at `lib.rs:2361`: *"a refused replay leaves the original session untouched."*
- **Fix** Snapshot `(self.history.clone(), self.redo.clone())` before the mutation and restore both on `Err`, or build the candidate history in a local and only assign after the rebuild succeeds.
- **Verification** Read `undo`/`redo`, `replay_to_kind` (`2440-2520`), `rebuild` (`2523-2620`), `execute_group` (`795-860`), `commit_state` (`1330-1360`). Grepped the tests: `a_refused_group_changes_nothing` (`lib.rs:4677`) asserts `s.undo().is_ok()` and `undo_with_nothing_to_undo_is_a_noop` asserts `Ok(false)`. No test exercises a *failing* undo. Not covered.

#### 4. `unmount mixer` leaves the arranger wiring stale: a remounted mixer gets no inputs, and the orphaned nodes keep reading files for the session's life

- **Severity** MAJOR
- **Location** `crates/host/src/lib.rs:1395-1408` (the `Unmount` cleanup), `lib.rs:1763-1766` (`wire_arranger`'s early return)
- **Trigger**

  ```text
  mount mixer channels=2 @0
  pool <dir>
  arrange add_track t0 @0
  arrange add_clip t0 c0 s1 0 4800 0 0 0 1.0 @0
  bounce 4800 /tmp/a.wav        # renders, wires the arranger, arrange_dirty := false
  unmount mixer
  bounce 10 /tmp/x.wav          # the render that actually applies the scheduled unmount
  mount mixer channels=2
  bounce 4800 /tmp/b.wav        # ← silence, no error
  ```

- **Wrong behaviour** The second bounce writes a silent file. The `Unmount` arm clears `pending_cords`/`player_mailbox`/`player_underruns`/`player_deferred`/`mixer_channels` but **not** `wired_tracks`, `arranger_underruns`, or `arrange_dirty`. The mixer's disposer removes only the mixer node, so the `ArrangerNode`s stay in the graph with their cords dropped (`Graph::remove_node` retains `c = c.from.0 != idx && c.to.0 != idx`). The remounted mixer is appended at the end of the node list, and `wire_arranger` returns immediately because `arrange_dirty` is still `false` — so nothing re-wires. The session reports a full arrangement and `underruns() == 0` while emitting nothing. The same omission leaks: the orphaned arranger nodes keep rendering and reading their pool files every block, and `arranger_underruns` still counts them.
- **Evidence** `lib.rs:1395-1408`:

```rust
            HostCommand::Unmount { plugin, .. } => {
                let r = self.engine.unmount(plugin);
                if *plugin == "mixer" {
                    // the mixer (the bus) is gone — clear the host's mixer-dependent
                    // state so a later engine command cannot reach a stale
                    // `wire_pending`/`wire_arranger` and panic (GLM-5.3 review #6).
                    self.pending_cords.clear();
                    self.player_mailbox = None;
                    self.player_underruns = None;
                    self.player_deferred = None;
                    self.mixer_channels = None;
                }
                r
            }
```

  `lib.rs:1763-1766`:

```rust
    fn wire_arranger(&mut self) -> Result<(), String> {
        if !self.arrange_dirty {
            return Ok(());
        }
```

  `arrange_dirty` is set `true` only in the `Arrange` arm (`lib.rs:1433`) and `execute_group` (`lib.rs:828`); it is cleared at the end of a successful `wire_arranger` (`lib.rs:1843`).
- **Fix** In the `Unmount { plugin: "mixer" }` arm, also tear down the arranger wiring the same way `wire_arranger` does — drain `wired_tracks`, `graph.remove_node` each id, clear `arranger_underruns` — and set `self.arrange_dirty = true` so a remounted mixer reconciles on its next render.
- **Verification** Read the `Unmount` arm, `wire_arranger` in full (`1763-1850`), `MixerPlugin::apply`'s disposer (`crates/engine/src/plugins/mixer.rs:558-565` — `remove_node` + `ctx.remove`), `Graph::remove_node` (`crates/engine/src/graph.rs:1024-1051`). Grepped: `unmount mixer` appears in exactly one test (`params_fold_from_the_log`, `lib.rs:565`), which has no arrangement and no remount. Not covered.

#### 5. `HostCommand::Take`'s channel count is unbounded — a 30-byte script line aborts the process

- **Severity** MAJOR
- **Location** `crates/host/src/lib.rs:1591-1597` (the allocation), `lib.rs:3431-3433` (the parse)
- **Trigger** `printf 'host v1\ntake jam 0 0 100000000000 0\n' | host` (or any saved session carrying that line, replayed on every `undo`/`seek`/`export`).
- **Wrong behaviour** `parse_script` accepts it; `apply` builds `(0..100_000_000_000).map(|k| format!("{take_id}.ch{k}")).collect()`. `Range`'s size hint is exact, so `collect` calls `Vec::with_capacity(n)` — either a `capacity overflow` panic or a multi-terabyte allocation and abort. Either way the host dies on malformed wire input, which is precisely the contract `tests/parse_script_robustness.rs` states (*"must be a clean `Err` … it cannot panic"*).
- **Evidence** `lib.rs:1588-1597`:

```rust
                self.status_take(&TakeReport {
                    take_id: take_id.clone(),
                    frames: *frames,
                    dropped: *dropped,
                    channels: *channels,
                    sources: (0..*channels).map(|k| format!("{take_id}.ch{k}")).collect(),
                    sample_rate: self.engine.clock.sample_rate,
                    at_frame: *at_frame,
                });
```

  and the parse, `lib.rs:3431-3433`:

```rust
                let channels = word(&words, 4, at)?
                    .parse::<usize>()
                    .map_err(|_| format!("line {at}: bad take channel count"))?;
```

  The sibling arms *are* bounded, so this is an inconsistency rather than a design choice: `SourceAdd` checks `1..=media::capture::CAPTURE_CHANNELS_SANITY` (`lib.rs:1625-1630`), `start_recording` is bounded by `media::Capture::start` (`crates/media/src/capture.rs:65-70`), and `Bounce` is bounded by `check_bounce_budget` — which is the fix the previous review's "minor" finding asked for, applied there and missed here.
- **Fix** Validate in the `Take` apply arm, mirroring `SourceAdd`: refuse `channels` outside `1..=CAPTURE_CHANNELS_SANITY` before building `sources`. A parse-side bound is better still, so the wire form refuses rather than applies.
- **Verification** Read the `Take` parse and apply arms, `SourceAdd`'s bound, `Capture::start`'s bound, `check_bounce_budget` (`lib.rs:1888-1913`). `a_take_declaration_replays_without_a_device` (`lib.rs:7219`) and `a_finished_take_is_committed_to_the_session_state` (`lib.rs:7183`) both use `channels == 2`. No large-channel test. Not covered.

#### 6. `live.rs`'s `Request::Load` replaces the live session with an empty one when the load fails

- **Severity** MAJOR
- **Location** `crates/host/src/live.rs:399-407`
- **Trigger** A live shell issues `HostHandle::load` with a script that has a typo, a missing pool, or an unparseable command.
- **Wrong behaviour** The caller gets `Err(e)`, but the running session has already been replaced by `HostSession::new()` — arrangement, pool binding, history and any unsaved edits are gone, and the shell keeps publishing an empty session. The command form of the same operation gets this right: `load_session` builds the candidate and only assigns on success (`lib.rs:1262-1278`, `*self = loaded` is the last statement), so the two paths disagree.
- **Evidence** `live.rs:399-407`:

```rust
                let mut applied = Ok(());
                let fresh = match HostSession::from_script(&commands) {
                    Ok(fresh) => fresh,
                    Err(e) => {
                        applied = Err(e);
                        HostSession::new()
                    }
                };
                let outcome = applied.map(|()| build_outcome(&fresh));
                session = fresh;
```

- **Fix** Keep the live session when the load fails: build the candidate, and only on `Ok` do `session = fresh; anchor = None;` plus the ring drain; return the `Err` and `publish` the *existing* session. (A take in progress should also be stopped-and-reported on the success path, as `replay_to_kind` does at `lib.rs:2468`.)
- **Verification** Read `live.rs:370-420` in full, `load_session` (`lib.rs:1262-1278`), and `live_host_loads_a_script_and_reloads_cleanly`. The test only exercises successful loads. Not covered.

### MINOR

#### 7. A refused `save` has already copied the pool, re-adopted it, and re-based the history

- **Severity** MINOR
- **Location** `crates/host/src/lib.rs:1179-1189` (the mutation) vs `lib.rs:1197-1221` (the check)
- **Trigger** Any save of a session whose history contains something the text form cannot round-trip, where the pool lives outside the target directory.
- **Wrong behaviour** `copy_pool` runs, `self.set_pool(target_pool)` re-adopts the copy, and `rebase_pool(&mut self.history, …)` rewrites every `Pool` command — all *before* the round-trip check that then returns `Err("…refusing to write a lossy session")`. The caller's natural reaction to that error (delete the directory it was told nothing was written to) removes the pool the session now depends on. The directory is also left on disk, so the test's own claim of *"nothing was written"* is only true of `session.txt`.
- **Evidence** `lib.rs:1179-1189`:

```rust
        if let Some(src) = self.pool_dir.clone()
            && src != target_pool
        {
            copy_pool(&src, &target_pool)?;
            // Point the live session at the copy **and re-base the log**: the pool is
            // context, and a replay (undo, seek) rebuilds from the history — if the
            // history kept the old path, an undo after a save would re-adopt it (and
            // fail outright if the original was moved or deleted).
            self.set_pool(target_pool.clone())?;
            rebase_pool(&mut self.history, &target_pool);
        }
```

- **Fix** Build the candidate text against a *clone* of the history with the would-be pool path applied, run the round-trip check on that, and only then perform the copy + `set_pool` + rebase.
- **Verification** Read `save` end to end (`1175-1260`), `rebase_pool` (`2843-2860`), `resolve_session_paths` (`1339-1352`). `a_save_that_cannot_round_trip_is_refused` (`lib.rs:6947`) uses a pool (`root/my pool`) distinct from the save target (`root/song.d/pool`), so the copy *does* run in that test — it just never asserts the session is unchanged.

#### 8. `process` renders up to `at_frame` *before* validating, so a refused frame-placed command moves the playhead

- **Severity** MINOR
- **Location** `crates/host/src/lib.rs:2716-2725`
- **Trigger** `set_param tone gain 1 @96000` on a session with no `tone` mounted.
- **Wrong behaviour** The session renders ~2 s of audio and the clock advances to frame 96 000, and *then* `engine.set_param` refuses. Nothing is logged, so invariant 3 holds — but `execute`'s contract is violated: *"A refused op returns `Err` and changes nothing; the session stays usable"* (`lib.rs:2340`). In a live shell the transport jumps two seconds on a rejected edit, and any stateful node (compressor envelope, limiter lookahead) has advanced.
- **Evidence** `lib.rs:2716-2725`:

```rust
        if let Some(frame) = cmd.at_frame() {
            let now = self.engine.clock.frame();
            if frame > now {
                // The pre-render wiring can fail (a later command after the
                // mixer was unmounted); a clean Err, never a panic in a host.
                self.render((frame - now) as usize)?;
            }
        }
        let r = self.apply(cmd);
```

- **Fix** Nothing cheap — the render *is* how the frame is placed. At minimum, make the contract honest: say that a refused frame-placed command may have advanced the transport, or pre-validate synchronously in `process` (the engine already validates; only the host-level refusals like "play requires the mixer" would need hoisting) and render only after that check.
- **Verification** Read `process` (`2690-2760`) and `apply` (`1382-1590`). `refused_commands_are_rejected_identically` (`crates/host/tests/reference_host.rs:71`) and `live_host_executes_commands_and_refusals_survive` all use `at_frame: Some(0)` or `None`, so `frame > now` is never true in a refusal test. Not covered.

#### 9. `replay_to_kind` discards `stop_recording`'s error with `let _ =`

- **Severity** MINOR
- **Location** `crates/host/src/lib.rs:2468-2470` (and the test-only twin at `lib.rs:2555`)
- **Trigger** Seek, undo, or redo while a take is recording and the finalization reports a complaint (a partial tail frame, a peaks write failure).
- **Wrong behaviour** `stop_recording` returns `Err` for exactly that case and the code throws it away. The take's `TakeReport` survives via `carry_over` (`lib.rs:2389-2392`), so the shell shows a take with no indication anything went wrong, contradicting the doc comment three lines above (*"the take is finalized … and reported"*).
- **Evidence** `lib.rs:2468-2470`:

```rust
        if self.recording.is_some() {
            let _ = self.stop_recording();
        }
```

- **Fix** Capture the error and surface it (e.g. append to `journal_error`, or keep it and return it after the rebuild succeeds) rather than discarding it.
- **Verification** Read `stop_recording` (`940-976`), `replay_to_kind` (`2440-2520`), `carry_over` (`2378-2420`). `recording_refuses_what_it_cannot_do` covers `RecordStop`'s propagation, not this path. Not covered.

### NIT

#### 10. The `group` arm is the only arm with no word-count check

- **Severity** NIT
- **Location** `crates/host/src/lib.rs:3259-3277`
- **Trigger** `group begin junk` / `group end junk` / `group begin @48000`.
- **Wrong behaviour** `group begin junk` and `group end junk` are accepted with the extra token silently discarded, and a trailing `@48000` on either line is stripped by the modifier loop and then dropped, because `HostCommand::Group` has no frame of its own. Every other arm routes through `exact()` (`lib.rs:3937-3946`), whose doc states the rule this breaks: *"an **extra** word is a typo, not something to ignore."* (`snap=` on a group line *is* refused, by the global check at `lib.rs:3531-3537` — so `@frame` being silent is an inconsistency within the same line.)
- **Evidence** `lib.rs:3259-3261`:

```rust
            "group" => match word(&words, 1, at)? {
                "begin" => {
                    if group.is_some() {
```

- **Fix** `exact(&words, 2, at, "group")?` before the sub-keyword match, and reject a non-`None` `at_frame` on a `group` line the way `snap=` already is.
- **Verification** Read the whole arm and the trailing-modifier loop (`lib.rs:3592-3610`); confirmed `group`'s `@frame` is never read.

#### 11. `copy_pool`'s "already identical" heuristic treats two unreadable mtimes as identical

- **Severity** NIT
- **Location** `crates/host/src/lib.rs:3154`
- **Trigger** `metadata()` succeeds on both sides but `modified()` fails on both (an exotic filesystem / restricted mount).
- **Wrong behaviour** `a.modified().ok() == b.modified().ok()` is `None == None` → `true`, so the file is treated as already copied and **not** refreshed — a stale pool source in a saved session.
- **Evidence** `lib.rs:3154`:

```rust
            .is_some_and(|(a, b)| a.len() == b.len() && a.modified().ok() == b.modified().ok());
```

- **Fix** Require both to be `Some` and equal: `a.modified().ok().zip(b.modified().ok()).is_some_and(|(x, y)| a.len() == b.len() && x == y)`.

### Test gaps

- No test combines `mount master` with `play` (finding 1) — the arranger path resolves the mixer by name, `wire_pending` does not, and nothing exercises the difference.
- No test journals a command whose `host v1` text form does not reparse (a path or matcher containing whitespace or `#`).
- No test for a **failing** `undo`/`redo`; only the success paths are asserted.
- No test for `unmount mixer` → remount with a wired arrangement, and none for the orphaned-node/reader-thread retention.
- No large-`channels` test for `take`; the bounded `bounce` sibling is covered but this arm is not.
- No test asserting a refused frame-placed command leaves `position().frame` unchanged.
- `parse_script_robustness.rs` covers `mount`/`set_param`/`set_tempo`/`unmount`/`play`/`splice`/`record`/`bounce`/`patch` only — nothing for `take`, `stretch`, `export`, `group`, `bounce`-frame, or huge numeric operands in any arm.
- `the_journal_autosaves_and_a_torn_line_is_dropped` writes only well-formed lines, so the parse-level journal failure is invisible.

### Design risks

- **A second MIDI port per session.** `midi_out_from_process()` runs inside `HostSession::new_at` (`lib.rs:621-622`), which `from_script` → `load_session` (`lib.rs:1263`) and live.rs's `Request::Load` both reach. The new device is opened *before* `*self = loaded` / `session = fresh` drops the old one, so a `load` transiently holds two handles on the same port. On an exclusive backend the second open fails and the failure is reported only on stderr (`lib.rs:2276-2280`) — the loaded session is silently clock-less. `rebuild`/`replay_to_kind` correctly avoid this via `new_at_with`; `from_script` does not.
- **`play`/`splice` paths are written absolute, `pool` is written relative.** `resolve_session_paths` (`lib.rs:1339-1352`) only rewrites `HostCommand::Pool`, and `format_command` writes `clip.path.display()` raw. `a_saved_session_can_be_moved` (`lib.rs:6591`) only exercises arrangement clips, so the *"self-contained and can be moved"* claim in `save`'s doc is true only for sessions without a `play`/`splice` — those fail to reopen after the move, with a bare file-open error.
- **The MIDI sink slot is a `std::sync::Mutex` locked from `render`.** `ClockOutNode::render` takes it per block (`crates/engine/src/plugins/clock_out.rs:282-290`) and `detach_midi`/`restore_midi` `.expect("the midi.out sink slot is not poisoned")` (`lib.rs:2508`, `lib.rs:2518`). A panic inside a `MidiSink::send` poisons the slot, and every subsequent host control operation then panics too. In the current runtime the "render" happens on the actor thread, so there is no audio-callback contention, but the shape is one `HostHandle` change away from invariant 1.
- **Every rebuild re-runs `Pool::conform` and re-warms every `play`.** `rebuild` replays the history, so each undo/redo/seek/export re-scans the pool and re-opens + `warm_player`s each clip (`warm_player` blocks up to 10 s, `lib.rs:1371-1380`). Undo latency scales with pool size and clip count.
- **`mixer_channels` is host shadow state duplicating the mixer's real width.** It is set in `process` from the mount params and cleared in the `Unmount` arm, while the authoritative width is `MixerNode::with_channels(self.channels, …)`. Nothing cross-checks the two, and `live.rs`'s `publish` iterates `0..mixer_channels()` against the meter's bank.

### Checked and clean

- **`parse_script` — the previous review's direct-index panics are genuinely fixed.** Every arm reaches its operands through `word()`, `get()`, or `first()`; `&words[2..]` in the `mount` arm is guarded by the preceding `word(&words, 1, at)?`; `&words[1..]` for `arrange` is reached only when `words[0] == "arrange"`. `exact()`'s `words.len() - 1` cannot underflow because `kind` is derived from `words.first()` and a non-`kind` match falls through to `other => Err`. `parse_snap`'s `token[5..]` and the `@` modifier's `tok[1..]` are char-boundary-safe because both guards are ASCII prefixes. The modifier loop cannot leave `kind` set with an empty `words` (an all-modifier line yields `kind == ""` → `unknown command ''`). `group` nesting, unterminated groups, and empty groups are all refused. The previous review's `at = lineno + 2` off-by-one is fixed: `header_line + lineno + 1` is correct with leading blank/comment lines.
- **Replay determinism.** No `HashMap` iteration order reaches the log, the script, or the render path: `wired_tracks` is only drained for `remove_node` (order-independent, since `index_of` searches by stable `NodeId`), `source_tempos` is sorted before publication in `live.rs:160`, and `params()` folds the ordered `Vec<Event>`. `Interner` order cannot change `Event` equality, since `&'static str` compares by content. `fmt_f32`/`fmt_f64` are round-trippable, and non-finite values are refused upstream (`validate_mount` `render.rs:229-235`, `set_param`, `parse_arrange`'s `f()` at `lib.rs:3694-3704`, `arrange_logged`'s `validate_op`), so NaN/inf cannot reach the log or the text form.
- **The clock-out wiring's interaction with the rest of the host is sound.** `detach_midi`/`restore_midi` bracket both `export` (`lib.rs:2007-2010`) and every `replay_to_kind` (`lib.rs:2461-2487`) and restore *before* the `?` in both. `rebuild` constructs the replacement with `Some(self.midi_slot.clone())` (`lib.rs:2506`), so the rebuild's `clock_out` node shares the slot and renders against the emptied one; `carry_over` re-establishes the same `Arc` (`lib.rs:2387`), so the live session is refilled rather than re-opened. Transport entries are fed from the apply path only, and `TransportPlay`/`TransportStop` are not `is_state()`, so a seek is silent by construction. The test-only `replay_full` twin duplicates the detach/restore shape correctly.
- **`Bounce` / `Export` ordering.** Render → write → log, with the non-finite scan and the `peak > 1.0` clip refusal before anything is logged, and `check_bounce_budget` bounding every allocation (`lib.rs:1888-1913`), including the `+ MAX_DRAIN_FRAMES` term for the drain.
- **`execute_group` atomicity.** The probe fold over a snapshot makes the gesture all-or-nothing; the residual window (a member that the timeline accepts but `arrange_logged` refuses) is documented at `lib.rs:820-830` and currently unreachable — the only `Value::F32` fields in arrangement ops are gains, which `validate_clip` (`crates/media/src/timeline.rs:331`) and the `SetClipGain` arm (`timeline.rs:741-743`) already reject as non-finite.
- **`stretch`.** The zero-denominator refusal (`lib.rs:1028`), the `MAX_STRETCH_RATIO` bound, the `min_region_frames` check and `MAX_STRETCH_FRAMES` gate all precede `try_reserve_exact` (`lib.rs:1105-1110`), so a malformed ratio or a 4 GiB WAV header yields a clean `Err`, not an allocation failure. The "render before log" ordering is deliberate and documented at `lib.rs:1013-1016`.
- **`load_session`** builds the candidate session and assigns `*self = loaded` only on success (`lib.rs:1262-1278`), so a failed `load` *command* leaves the live session intact — the contrast that makes finding 6 a bug rather than a design choice.
- **Two-phase discipline for the state commands.** `Mount`/`Patch`/`SetParam`/`SetTempo`/`Unmount` reach the engine (which validates synchronously) before `commit_state`; `Pool` calls `set_pool` before logging; `ArrangerNode::new` re-checks the source rate and clip invariants (`crates/media/src/arranger.rs:128-146`); `Play` opens and warms the clip before `arrange_logged`; `Splice` is refused without a player.

---

## 2. Adversarial verification (verbatim)

_No verification run for this scope._
