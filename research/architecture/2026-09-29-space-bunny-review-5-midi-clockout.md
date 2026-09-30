# Space-bunny external review — MIDI clock-out slice (752f96b, 49e8774)

> **Disposition.** External deep review, 2026-09-29, against `49e8774` (clean tree). Reviewer:
> Space Bunny (`opencode/space-bunny-free`) driven through OpenCode by DeepSeek Harness; the
> supervisor (DeepSeek-V4.1-Flash) wrote the scope, then handed every CRITICAL/MAJOR claim to an
> **independent adversarial verifier** that had to falsify it before it counted.
>
> This file is the **MIDI clock-out slice (752f96b, 49e8774)** scope, verbatim: the reviewer's report, then its verifier's
> verdicts. What was done about each finding — fixed, deferred or refuted — is in
> [the index](2026-09-29-space-bunny-review.md). Findings are the reviewer's, not the
> supervisor's: read the verdicts before acting on any single claim.

---

## 1. Reviewer's report (verbatim)

## MIDI clock-out slice review (752f96b, 49e8774, engine half 36cfd95)

### CRITICAL

1. **An accepted `set_tempo` below ~6.5e-15 bpm hangs the render thread forever — the tick loop is unbounded**

- **Severity**: CRITICAL (invariant 1: the render path must be bounded; a permanent process hang is worse than a panic)
- **Location**: `crates/engine/src/plugins/clock_out.rs:206-215` and `:264-274`; enabler at `crates/engine/src/clock.rs:99-112`
- **Trigger**: any session that mounts `clock_out` and applies a tempo the host accepts. `parse_script` accepts any f64 (`lib.rs:3317-3329`) and `Engine::set_tempo` only checks `is_finite() && bpm > 0.0` (`crates/engine/src/render.rs:499`). So `set_tempo 0.000000000000001 4` in a script, a `session.txt`, or a `journal.txt` reaches the tempo map.
- **Wrong behaviour**: `TempoMap::frame_at` documents its tail as "Unreachable", but it is reachable:
  ```rust
  // crates/engine/src/clock.rs:95-112
  let seg_frames = self.segments.get(i + 1)
      .map(|s| s.start_frame - seg.start_frame).unwrap_or(u64::MAX);
  let seg_beats = seg_frames as f64 * seg.bpm / 60.0 / self.sample_rate as f64;
  if remaining < seg_beats { … return seg.start_frame + frames; }
  remaining -= seg_beats;
  …
  // Unreachable: the final open-ended segment covers any finite beat.
  self.segments.last().expect("tempo map never empty").start_frame
  ```
  With one open segment, `seg_beats = 6.405e12 × bpm`; for `bpm ≲ 6.5e-15` that is below `1/24`, so `frame_at(n/24)` falls through and returns `0` for every `n`. Then:
  ```rust
  // crates/engine/src/plugins/clock_out.rs:206-214
  let mut n = (map.beat_at(frame) * TICKS_PER_BEAT as f64).floor() as u64;
  while Self::tick_frame(map, n) < frame { n += 1; }
  ```
  spins forever for any `frame > 0`; and for `frame == 0` the main loop does, because `tick_frame` is pinned at `0` and never reaches `block_end`:
  ```rust
  // crates/engine/src/plugins/clock_out.rs:264-274
  let mut n = Self::first_tick_at_or_after(block.tempo, block.frame);
  loop {
      let frame = Self::tick_frame(block.tempo, n);
      if frame >= block_end { break; }
      self.push(ExternalEvent::Clock { offset: (frame - block.frame) as u32 });
      n += 1;
  }
  ```
  The actor thread never returns from `render`, so the shell freezes, no further command is serviced, and `overflows` climbs forever. Opening the session and pressing play wedges the process permanently.
- **Evidence**: `overflow_is_counted_never_grown` (clock_out.rs:538-557) uses 1e9 bpm, where `seg_beats ≈ 6.4e21` and `frame_at` still resolves — the test sits far above the threshold and cannot see this. No test uses a small bpm.
- **Fix**: two independent guards. (a) Bound the loop by the declared cap, which the doc already promises ("when a block would exceed the bound, the node emits what fits and counts the rest"): stop after `n - n0 >= CLOCK_OUT_CAP` and `fetch_add` the remainder to `overflows`, and cap `first_tick_at_or_after`'s upward walk the same way. (b) Make `TempoMap::frame_at` return `u64::MAX` (not `last().start_frame`) when it falls through, and give the host a tempo floor (`bpm < 1e-3` → refuse, alongside the existing `set_tempo` validation).
- **Verification**: re-derive the threshold in `render.rs:499` (`bpm < 2.88e6 / (24 × 2^64) ≈ 6.5e-15` at 48 kHz); confirm nothing between `parse_script` and `TempoMap::push` clamps or rejects; confirm the existing 1e9-bpm overflow test still passes with the cap applied. A regression test with `set_tempo 1e-15 4` + `clock_out` + one render would have caught it.

### MAJOR

2. **A seek while playing never re-syncs the follower — the gear ends up permanently out of phase, and the note's "replay re-feeds the tap" claim is false**

- **Severity**: MAJOR (defeats the feature's stated purpose; doc-vs-code mismatch in the implemented note)
- **Location**: `crates/host/src/lib.rs:1409-1435` (the tap), `:440-457` (`is_state`), `:680` (fresh log per session), `:2464-2499` (`replay_to_kind`)
- **Trigger**: mount `clock_out` with a device, `play`, render 48 000 frames, then seek to a different frame **without stopping**. `TransportPlay`/`TransportStop` are actions, not state:
  ```rust
  // crates/host/src/lib.rs:440-446
  fn is_state(&self) -> bool {
      matches!(self,
          HostCommand::Mount { .. } | HostCommand::Patch { .. } | … )
  }
  ```
  so they never enter `history` and `rebuild` never re-applies them. And every rebuilt session gets a brand-new log:
  ```rust
  // crates/host/src/lib.rs:680
  let transport = Arc::new(TransportLog::new());
  ```
  `carry_over` (lib.rs:2524-2546) does not carry `transport` either.
- **Wrong behaviour**: after the seek the adopted session is still `playing` (rebuild sets `rebuilt.playing = self.playing`, lib.rs:2417), so the pump keeps rendering and the node emits ticks for the new position — but **no `Start`, no `Continue`, nothing**. A pulse-counting follower that received ticks 0…47 now receives tick 24 onward: it counts 24 pulses and believes it is at 72 while the session is at 48. A permanent half-second phase error at 120 bpm, with no message that could ever correct it, because the transport is already "playing" so no `play` command follows. The note says "nothing is sent across a rebuild, and **the next `play` re-syncs** (`Start`, or `Continue` after a pause) at that frame" — a mid-playback seek has no next play, so nothing re-syncs. The note's rationale "Feeding it in apply is what makes a replay re-feed the tap identically" is simply not true: no replay ever re-feeds the tap.
- **Evidence**: the shipped test *pins the absence* — after the seek it asserts `transport_events(&sends) == vec![(0, "Start")]` and then only checks tick **frames** after the post-seek render (lib.rs:3690-3723 area, `a_seek_sends_nothing_while_it_rebuilds_and_resumes_afterwards`). So the test cannot fail, and a fix will require updating it.
- **Fix**: make the rebuild itself the re-sync point. In `replay_to_kind`, after `restore_midi` and before `carry_over`, push into the **rebuilt** session's log: `rebuilt.transport.push(frame, if frame == 0 { Start } else { Continue })` when `self.playing` was true. The node then emits it on the first post-rebuild render, inside the session, at the right frame. Also correct the note sentence about replay re-feeding the tap, and either add `TransportPlay`/`Stop` to the rebuilt state or state plainly that the tap is deliberately not replayed.
- **Verification**: read `replay_to_kind` end to end, `is_state`, `format_command`'s action list (lib.rs:2971-2983), and `new_at_with`'s fresh `TransportLog`; confirmed no other path pushes into a rebuilt session's log. Confirmed undo/redo at the *current* frame is phase-benign (`first_tick_at_or_after(frame)` yields the tick the gear was next expecting), which is why the defect is seek-specific.

3. **`take_due` grows `transport_scratch` on the render path — the sibling `push()` is capped, this one is not**

- **Severity**: MAJOR (invariant 1: allocation-free render path; and the function's own doc claims otherwise)
- **Location**: `crates/engine/src/plugins/clock_out.rs:108-120`, used at `:242`; the guarded counterpart at `:188-195`
- **Trigger**: more than `CLOCK_OUT_CAP` (64) transport commands pending in one block. They are pushed from the apply path with no bound, and nothing renders between them: a script such as `mount clock_out @0` followed by 70 `transport play`/`transport stop` lines and a `bounce` (each `TransportPlay`/`Stop` has `at_frame() == None`, so `process` pre-renders nothing, lib.rs:2716-2723), or ≥65 transport commands issued to a live `HostHandle` while stopped (the actor renders only `if session.is_playing()`, live.rs:425, so the log accumulates).
- **Wrong behaviour**:
  ```rust
  // crates/engine/src/plugins/clock_out.rs:112-118
  pub fn take_due(&self, block_end: u64, out: &mut Vec<(u64, Transport)>) {
      let mut queue = self.queue.lock().expect("the transport log is not poisoned");
      let at = queue.partition_point(|&(f, _)| f < block_end);
      out.extend(queue.drain(..at));
      drop(queue);
  }
  ```
  `out` is `self.transport_scratch`, `Vec::with_capacity(CLOCK_OUT_CAP)`, cleared each block, so `extend` of 130 `Drain` items reallocates **inside `ClockOutNode::render`**. The doc on line 111 states "the node reuses a preallocated buffer, so draining allocates nothing", and the field doc says "the render path never grows it (see `CLOCK_OUT_CAP`)" — the guard exists on the event scratch (`push`, line 190: `if self.scratch.len() < self.scratch.capacity()`) and is simply missing here.
- **Evidence**: the counting-allocator proof (`crates/engine/tests/spike_a.rs:834-864`) mounts `clock_out` but provides **neither** a sink nor a transport log, so `take_due` is never entered — the test cannot see this. The module tests never queue more than 4 entries.
- **Fix**: mirror `push` — drain into a bounded loop and `fetch_add` the remainder to the same `overflows` counter, or make `take_due` take a `cap: usize` and stop there. Then add a test that queues `CLOCK_OUT_CAP + 1` transport entries and asserts `emitted + overflows == due`, the way `overflow_is_counted_never_grown` does for ticks.
- **Verification**: confirmed `CLOCK_OUT_CAP == 64`, both scratch buffers are `with_capacity(CLOCK_OUT_CAP)` (clock_out.rs:176-177), `scratch.clear()`/`transport_scratch.clear()` precede the drain (clock_out.rs:232-233), and `Vec::extend` over a `Drain` (a `TrustedLen` iterator) calls `reserve` and reallocates. Confirmed no test exercises >4 pending entries.

4. **The sink's `dropped_events`/`dropped_bytes` are unreachable in the product — a stalled writer is silent, though the code says "read by the host"**

- **Severity**: MAJOR (the only signal that the realtime guarantee failed is dead; doc-vs-code mismatch)
- **Location**: `crates/media/src/midi.rs:249-258` (the counters and their doc), `crates/host/src/lib.rs:2271-2277` (where the concrete type is erased)
- **Trigger**: the ring fills while the writer stalls (a wedged thread, a device that stops draining, a machine under load) while `clock_out` is mounted with a device.
- **Wrong behaviour**: the module documents the counters as the loud bound — "the loud bound, **read by the host** (a nonzero count means the writer stalled)" — and the header says drops are counted "exactly how capture counts an overrun". But the host boxes the sink straight into a trait object:
  ```rust
  // crates/host/src/lib.rs:2271-2277
  match MidiOut::open(&port) {
      Ok(out) => (Some(port), Some(Arc::new(Mutex::new(Some(
          Box::new(out) as Box<dyn MidiSink>
      ))))),
  ```
  and `MidiOutStatus { port, overflows }` reports only the **plugin's** `CLOCK_OUT_OVERFLOWS_KEY` (lib.rs:2168-2178), which counts `CLOCK_OUT_CAP` evictions, not ring drops. `grep` across the workspace finds `dropped_events`/`dropped_bytes` only inside `midi.rs` — the unit tests and the `#[ignore]`d hardware test. So in a running host a stalled writer drops clock silently: the snapshot says `{ port: Some("um-1"), overflows: 0 }`.
- **Evidence**: `crates/host/src/live.rs:593-599` and `Snapshot::midi` (live.rs:60-67) are the whole read path; neither mentions ring drops. The note's consequence list ("drops are counted") describes compiled-but-unreachable code.
- **Fix**: keep the `Arc<AtomicU64>` pair in the session (`MidiOut` already shares them by `Arc`) — e.g. have `MidiOut::open` hand back `(MidiOut, Arc<DropCounters>)` or add `fn counters(&self) -> Arc<MidiDropCounters>` — store it in `HostSession`, and add `MidiOutStatus { ring_drops, ring_dropped_bytes }`; print it in `main.rs` and publish it in the snapshot. Alternatively, downcast the trait object — no, the `Arc` is cleaner.
- **Verification**: read the whole of `midi.rs`, the `MidiOutStatus` construction, and the snapshot publish; grepped every `.rs` in the workspace for the two method names. Confirmed `ports()` is likewise only used by the `#[ignore]`d test.

5. **`Bounce` renders with the sink attached — the offline bounce (and its drain tail) drives gear, which the note rejects for `export` on exactly these grounds**

- **Severity**: MAJOR (an undeclared hardware side effect on the reference host's headline path)
- **Location**: `crates/host/src/lib.rs:1684-1693`; contrast `:2002-2011`
- **Trigger**: any script or shell that runs `bounce` with `--midi-out`/`DSH_MIDI_OUT` set and `clock_out` mounted — i.e. `run_script`'s own documented shape, "the final `Bounce` writes the master".
- **Wrong behaviour**:
  ```rust
  // crates/host/src/lib.rs:1684-1688
  HostCommand::Bounce { frames, path } => {
      // Offline bounce: render + drain buffered tails. …
      let (out, drain) = self.render_with_drain(*frames)?;
  ```
  No `detach_midi`, unlike `export` (`:2007-2010`). And the node ignores `block.mode`, so `RenderMode::Drain` blocks advance the clock and emit ticks too (clock_out.rs:223-291 never reads `block.mode`; `Engine::drain` advances the clock at render.rs:1052). `HostSession::render_with_drain` also goes through `render_with_drain_aligned` (render.rs:988-1006), which renders `frames + flush_frames()` — the graph's PDC transit is trimmed from the **audio** but not from the **clock**, so a bounded number of extra ticks lead the file. Net wire effect: `Start`, ticks for the whole bounce plus its drain tail, and no `Stop`.
- **Evidence**: the note (`2026-09-29-midi-clock-out-in-the-host.md`, "Let an export drive gear") states the principle: "an offline render exists to produce a file, and driving hardware from it is a side effect nobody asked for. The export path detaches for both its rebuild and its render." `Bounce` was evidently not considered — it is neither in the note nor in the `detach_midi` call sites (lib.rs:2007, 2483, 2559).
- **Fix**: wrap the `Bounce` arm the same way `export` is wrapped — `let device = self.detach_midi(); let r = …; self.restore_midi(device); r` — or, better, hoist the detach into `process`/`apply` around every offline render (`Bounce`, `Export`) so the invariant lives in one place.
- **Verification**: read the `Bounce` arm in full, `export`/`export_detached`, `Engine::drain`, and `render_with_drain_aligned`; confirmed the node's `render` takes `block: RenderBlock` and uses only `block.frame`/`block.tempo`, never `block.mode`. The existing `an_export_clone_sends_nothing` test covers `Export` only.

6. **The node takes two `Mutex` locks and two `.expect()`s on the render path — invariant 1 says lock-free and no panic paths**

- **Severity**: MAJOR (direct violation of a stated invariant; a panic here is on the thread that renders audio)
- **Location**: `crates/engine/src/plugins/clock_out.rs:241-257` and `:282-290`
- **Trigger**: every block in which the transport log is present, and every block in which `scratch` is non-empty (i.e. always, once a device or transport is mounted).
- **Wrong behaviour**:
  ```rust
  // crates/engine/src/plugins/clock_out.rs:241-242
  if let Some(log) = &self.transport {
      log.take_due(block_end, &mut self.transport_scratch);
  ```
  and
  ```rust
  // crates/engine/src/plugins/clock_out.rs:282-290
  if !self.scratch.is_empty()
      && let Some(device) = self
          .sink
          .lock()
          .expect("the midi.out sink is not poisoned")
          .as_mut()
  {
      device.send(&self.scratch, block.frame);
  }
  ```
  Two `std::sync::Mutex` acquisitions, both with `.expect` panic paths, now sit in the graph's render callback. The control thread contends for the sink mutex in `detach_midi`/`restore_midi` (lib.rs:2505-2519) and for the log mutex in `TransportLog::push` (clock_out.rs:99-106) — and `push` does a `Vec::insert` that can reallocate and copy, so the render thread can block on it for the duration of a copy. The `MidiOut::send` half of the claim is honest (I verified it: `to_wire` is pure and returns a 4-byte `Copy`, `Spsc::try_push` is allocation-free and non-blocking, and `dropped_events`/`dropped_bytes` are incremented exactly once per dropped message with `msg.len` bytes); the *node* wrapped around it is neither lock-free nor panic-free.
- **Evidence**: `send`'s own doc claims the property at the wrong layer ("The render path's only entry point: convert, push, count. No lock, no allocation, no device" — midi.rs:275-277), which is true of `MidiOut` and false of `ClockOutNode::render`. No test measures locks or panics; the counting-allocator test only counts allocations.
- **Fix**: replace the sink mutex with an `AtomicBool` "armed" flag plus a `Mutex` the *control* side alone touches — the node does `if slot.armed.load(Relaxed) { /* only then */ let mut g = slot.m.lock(); … }`, so a rebuild costs one relaxed atomic load instead of a lock. Same for the transport log: publish it as an `Arc<TransportLog>` whose mutex is replaced by a fixed-capacity `Spsc<(u64, Transport)>` drained lock-free (the repo already has that ring, and it is the right shape for a control-producer/render-consumer queue). If the mutexes stay, `.expect` must become a silent skip (`if let Ok(mut g) = …`) so no panic can reach the render thread.
- **Verification**: read `ClockOutNode::render` in full plus both mutex owners (`detach_midi`/`restore_midi`, `TransportLog::push`) and confirmed the control side never holds either lock across a render, so the exposure is contention latency, not deadlock. Confirmed `MidiSink: EventSink: Send` (plugins/mod.rs:143-151) and `Spsc<Wire>` is `Sync` (ring.rs:30), so the ring's single-producer contract holds: `Engine::mount` refuses a duplicate name (render.rs:226), and only one node ever renders at a time because the actor is single-threaded and a rebuild renders while the old session is idle.

### MINOR

7. **Every `Load` opens a second MIDI port (second ALSA connection + second writer thread) and silently drops to no clock-out if the second `connect` fails**

- **Severity**: MINOR
- **Location**: `crates/host/src/lib.rs:2326` (`from_script` → `new_at`), `:621-624` (`new_at` → `midi_out_from_process`)
- **Trigger**: a shell `load` with `--midi-out`/`DSH_MIDI_OUT` set.
- **Wrong behaviour**: `from_script` calls `HostSession::new_at(rate)`, which re-reads the process config and calls `MidiOut::open` again; `live.rs:395-416` then drops the old session, closing the first port and joining its writer. A second `connect` to the same ALSA seq port can be refused by the backend, and `midi_out_from_process` then takes the silent branch:
  ```rust
  // crates/host/src/lib.rs:2278-2282
  Err(e) => { eprintln!("host: the requested MIDI output '{port}' could not be opened: {e}");
             eprintln!("host: continuing without a MIDI output (clock-out stays silent)");
             (None, None) }
  ```
  The loaded session then has `midi.port == None` and never drives gear again, with only two stderr lines. Two writer threads briefly coexist.
- **Evidence**: `rebuild`/`rebuild_prefix` correctly pass the shared slot via `new_at_with` (lib.rs:2388-2392, 2577-2581), so only `from_script` leaks; `HostHandle::spawn` → `HostSession::new()` (live.rs:250) is the other entry.
- **Fix**: make the process-level sink a lazily-initialised process singleton (`OnceLock<Option<SharedMidiSink>>` keyed by the requested port) that `new_at` *clones the Arc of* rather than re-opening, so a rebuild and a load share one connection and one writer thread. Alternatively thread the existing slot through `from_script` the way `rebuild` does.
- **Verification**: read `from_script`, `new_at`, `new_at_with`, `rebuild`, `rebuild_prefix`, and the actor's `Load` arm; confirmed the only double-open path is `from_script`.

8. **`midi_status().port == None` conflates "no device requested" with "the device you asked for failed to open", and `main.rs` prints nothing in the failed case**

- **Severity**: MINOR
- **Location**: `crates/host/src/lib.rs:2278-2282`, `:2168-2178`; `crates/host/src/main.rs:139-145`
- **Trigger**: `--midi-out` naming a port that does not exist, or a typo'd substring.
- **Wrong behaviour**: the note's acceptance criterion 4 says the session "reports that rather than failing"; the only report is stderr, and the snapshot/`main.rs` line are indistinguishable from "no gear configured":
  ```rust
  // crates/host/src/main.rs:139-145
  match (&midi.port, midi.overflows) {
      (Some(port), 0) => println!("host: midi out: {port} (no overflows)"),
      (Some(port), n) => println!("host: midi out: {port} — OVERFLOWED {n} events"),
      (None, _) => {}
  }
  ```
  A shell rendering `Snapshot::midi` cannot tell a misconfigured rig from an idle one.
- **Evidence**: `MidiOutStatus` has two fields and no error/state discriminant (lib.rs:485-495).
- **Fix**: carry the failure in the status — e.g. `MidiOutStatus { port, requested: Option<String>, error: Option<String>, overflows }` — and print it in `main.rs` on the `None`-with-`Some(requested)` case.
- **Verification**: read `midi_status`, `MidiOutStatus`, the `main.rs` match, and the `Err` arm of `midi_out_from_process`.

9. **`overflows` silently resets to 0 on every rebuild, so the loud counter's history is lost exactly when a seek hides the fact that ticks were dropped**

- **Severity**: MINOR
- **Location**: `crates/host/src/lib.rs:2168-2178` read through the rebuilt engine's context; `crates/engine/src/plugins/clock_out.rs:330-332`
- **Trigger**: any seek, undo or redo after an overflow, or after an export (whose detached render still generates ticks and can still overflow).
- **Wrong behaviour**: the plugin publishes a **fresh** `Arc<AtomicU64>` per apply and `rebuild` builds a new engine, so `midi_status().overflows` restarts at 0. The one loud signal for "clock was dropped" is a per-session-lifetime value that a seek clears.
- **Evidence**: `apply` does `let overflows = Arc::new(AtomicU64::new(0)); api.ctx.provide(CLOCK_OUT_OVERFLOWS_KEY, overflows);` (clock_out.rs:330-332); `rebuild` calls `new_at_with`, and `*self = rebuilt` (lib.rs:2497) swaps the engine.
- **Fix**: allocate the counter once in `new_at_with` (like the slot) and `provide` that same `Arc` in every `apply`, so it is process/session-lifetime. Or accumulate into a session-level `Arc<AtomicU64>` that `midi_status` reads instead of the context.
- **Verification**: read `apply`, the disposer (`dis.ctx.remove(CLOCK_OUT_OVERFLOWS_KEY)`, clock_out.rs:339), `rebuild`, and `midi_status`; confirmed nothing carries the counter across. Confirmed the overflow test only reads the counter within one session.

10. **README still says MIDI clock out is a stub and MIDI is a seam with no implementation**

- **Severity**: MINOR (AGENTS.md: "Keep paths, names, and defaults current in the same change that alters them")
- **Location**: `README.md:18` and `README.md:68-71`
- **Trigger**: reading the README after this slice.
- **Wrong behaviour**: both claims are now false. Line 18: "capture and takes work; clock-out and alignment are not built". Line 68-71: "The recorder's two defining capabilities — **MIDI clock out** and **alignment of takes** — are stubs … MIDI and OSC are declared seams with no implementation". `clock_out` is in `HOST_PLUGINS` (lib.rs:57), `MidiOut` is a real `midir` sink, and the snapshot carries `midi`. `docs/capabilities.md` does not mention MIDI at all, and neither shell displays the new field.
- **Evidence**: `grep -i midi docs/` finds only `theory-of-the-program.md:87` and `architecture-explainer.md:1080` ("Some seams are *declared* but not yet exercised (MIDI/OSC traits in `plugins`)") — the latter is also now stale for MIDI.
- **Fix**: update the profile table and the "Not yet" paragraph (clock out ships, gated on `--midi-out`; alignment is still a stub), refresh the `Last verified against commit 4a3706f` banner, and either surface `Snapshot::midi` in the shells or say the field is API-only.
- **Verification**: read both README lines; grepped `docs/` and `spikes/*/src` for `midi` (no hits in the shells).

11. **`DSH_MIDI_OUT` leaks into the whole test suite — with it set, every `HostSession::new()` opens a real port and one test fails**

- **Severity**: MINOR
- **Location**: `crates/host/src/lib.rs:2267` inside `new_at` (`:621`), reached by `Default for HostSession`, `HostHandle::spawn` (live.rs:250) and `from_script`
- **Trigger**: a developer with `DSH_MIDI_OUT=um-1` exported runs `cargo test -p host`.
- **Wrong behaviour**: every session construction in every test allocates a `MidiOut`, spawns a writer thread and opens a real ALSA port; and `the_snapshot_publishes_the_midi_status` asserts the *absence* of a port, so it fails:
  ```rust
  // crates/host/src/live.rs:735-741
  assert_eq!(s.midi, crate::MidiOutStatus::default(),
      "no port asked for, no overflows at a sane tempo");
  ```
  CI is green because CI does not set the variable, so the failure is invisible until it is not.
- **Evidence**: `midi_out_from_process` is called from `new_at` unconditionally, before any knowledge of whether it is a test.
- **Fix**: take the sink from an explicit constructor argument and have only `main.rs`/the shells call the process-config reader — or have `new_at` consult the config only when an `opt_env("DSH_...")`-style test gate is off, as the other hardware tests do.
- **Verification**: read `new_at`, `new_at_with`, `midi_out_from_process`, the `Default` impl, and the live test; confirmed no test-time override exists.

12. **`MidiOut::drop` can discard up to a full ring of queued messages, and does not count them**

- **Severity**: MINOR
- **Location**: `crates/media/src/midi.rs:220-247` (writer loop), `:261-268` (`Drop`)
- **Trigger**: `MidiOut` dropped immediately after pushes — e.g. quitting the app right after a `TransportStop`.
- **Wrong behaviour**: `stop` is set before the writer necessarily observes it, and the loop's condition is checked *before* the drain:
  ```rust
  // crates/media/src/midi.rs:224-237
  while !stop2.load(Ordering::Relaxed) {
      let mut sent = false;
      while let Some(msg) = ring2.try_pop() { if let Err(e) = conn.send(msg.as_slice()) { … } sent = true; }
      if !sent { std::thread::sleep(WRITER_PARK); }
  }
  ```
  If `stop` lands before the first iteration, up to 1024 queued messages — typically including the trailing `Stop` — are dropped on the floor and **not** added to `dropped_events`, so the counters under-report. There is also no All-Notes-Off, so any future note output would hang on the gear (nothing emits notes today — see the design risks).
- **Evidence**: `dropped_events` is only incremented in `send` (midi.rs:285), never on teardown.
- **Fix**: restructure the loop as `loop { drain; if stop { break } if !sent { sleep } }`, and add the discarded count (or a `flush_on_drop` that attempts one final drain) to `dropped_events`.
- **Verification**: read the writer closure and `Drop` in full; confirmed `Drop` only sets the flag and joins.

13. **`midir` is a git dependency with no `rev`/`tag` in the manifest — the "pinned by Cargo.lock" claim holds only until someone regenerates a lock**

- **Severity**: MINOR
- **Location**: `crates/media/Cargo.toml:17`
- **Trigger**: `cargo update -p midir`, a new lock file, or the three lock files drifting (exactly the class `49e8774` had to fix for the spikes).
- **Wrong behaviour**:
  ```toml
  # crates/media/Cargo.toml:17
  midir = { git = "https://github.com/Boddlnagg/midir" }
  ```
  No `rev`, so the manifest alone does not pin anything; the three committed locks do. The note says "Pinned by Cargo.lock and commented" — accurate but fragile, and the fix commit's own lesson is that this repo has three lock files.
- **Evidence**: the note itself (`2026-09-29-midi-clock-out-in-the-host.md`, "Alternatives considered") calls it "a **git dependency on unreleased 0.11**, pinned by `Cargo.lock` and commented".
- **Fix**: add `rev = "<40-hex>"` to the manifest so the pin travels with the dependency, and note in the note that the three locks must be regenerated together (which `49e8774` already established).
- **Verification**: read `crates/media/Cargo.toml` and the note's dependency paragraph; confirmed no `rev`/`tag`/`branch` key.

### NIT

14. **The writer's stop latch is `Relaxed` on both sides, but `MidiOut::drop` depends on it being observed**
    - **Location**: `crates/media/src/midi.rs:224` (writer's load), `:263` (`drop`'s store)
    - `Ordering::Relaxed` is the house pattern for *counters*, but this is a latch whose visibility `join()` depends on. Formally a relaxed load may keep returning the old value, which would make `Drop`'s `join()` unbounded. In practice the loop body contains the ring's `Acquire` load, so the compiler must re-issue it and it converges — hence a NIT, not a finding. Use `Release` on the store and `Acquire` on the load. (`crates/media/src/capture.rs`, `stream.rs`, `record.rs` use `Relaxed` only for counters, so this is not house style for latches.)

15. **`carry_over`'s slot assignment is a no-op today and a latent bug if `rebuild` ever stops sharing the arc**
    - **Location**: `crates/host/src/lib.rs:2527-2531`
    - `rebuild`/`rebuild_prefix` both pass `Some(self.midi_slot.clone())`, so `self.midi_slot = from.midi_slot.clone()` assigns the same `Arc`. If a future rebuild path constructed a distinct slot, this line would move the old session's `Box<dyn MidiSink>` into the rebuilt one and **drop the rebuilt slot's device** — a silent "gear dead after seek". The comment ("in practice the rebuilt session already shares it") acknowledges the redundancy. Prefer a `debug_assert!(Arc::ptr_eq(...))` plus no assignment, or drop the line.

16. **A failed device send `eprintln!`s once per message — unbounded stderr spew at clock rate**
    - **Location**: `crates/media/src/midi.rs:229-231`
    - If a port is unplugged mid-session, `conn.send` fails for every tick and the writer prints 48+ lines/second forever, with no backoff and no latch on "already reported". Rate-limit to first-N-then-count, mirroring the ring's drop counter.

17. **`to_wire`'s `NoteOn`/`NoteOff`/`Control` arms are unreachable in production**
    - **Location**: `crates/media/src/midi.rs:108-121`
    - `clock_out` emits only `Clock`/`Start`/`Stop`/`Continue` (clock_out.rs:250-274), so three of the six arms — and the whole documented pitch/velocity/CC conversion surface, the `Control`→CC#1 choice the note records as a limitation, and the note-off-vs-note-on-velocity-0 question — are exercised only by `notes_and_control_use_the_documented_conversions` (midi.rs:341-378). Not wrong, but the wire contract for notes is asserted against nothing real, and the note's own consequence list ("`Control` maps to CC#1 … recorded rather than hidden") describes a path with no producer.

18. **The pitch conversion is quiet about NaN**
    - **Location**: `crates/media/src/midi.rs:96-98`
    - `(pitch + 69.0).round().clamp(0.0, 127.0) as u8` — `f32::clamp` returns NaN unchanged, and `NaN as u8` saturates to `0`, so a NaN pitch becomes MIDI note 0 (a very low note) rather than a refusal. Not reachable today (pitch comes from the engine's own scale generator), but a one-line `if !pitch.is_finite() { return 0 }` or a `debug_assert!` would keep the wire honest. Same for `to_7bit` (line 103-105) where NaN becomes velocity 0 — i.e. a silent note-off.

### Test gaps

- **No allocation test with a transport log present.** `render_path_does_not_allocate` (`crates/engine/tests/spike_a.rs:834-864`) mounts `clock_out` with neither a sink nor a `TRANSPORT_KEY` service, so `take_due` (finding 3) is never entered. A variant that provides a `TransportLog` and queues >64 entries would have found it.
- **No test for a small/absurd tempo.** The overflow test uses 1e9 bpm; nothing covers the ~6.5e-15 threshold where `frame_at` falls through (finding 1). One `set_tempo 1e-15 4` + render would have caught a permanent hang.
- **`tempo_change_inside_a_block_stays_exact` is tautological about the tick frames.** Its expectation is built from the function under test:
  ```rust
  // crates/engine/src/plugins/clock_out.rs:495-499
  let expected: Vec<u64> = (0..)
      .map(|n| ClockOutNode::tick_frame(&map, n))
      .take_while(|&f| f < 48_000)
      .collect();
  assert_eq!(FakeSink::clock_frames(&recorded), expected);
  ```
  It pins *which* `n` are visited, not *where* they land. A hand-computed list (or a second, independent beat→frame formula) is needed for the "to the sample" claim across a tempo change.
- **No test asserts a transport message after a mid-playback seek** — the shipped test asserts the opposite (finding 2), so a fix must update it rather than add to it.
- **No test that `bounce` leaves gear alone** (finding 5); `an_export_clone_sends_nothing` covers `Export` only.
- **No test that the ring-drop counters are visible anywhere** — because they are not (finding 4). The `#[ignore]`d hardware test asserts `dropped_events() == 0` from inside the module, which proves the counter works, not that the product can see it.
- **No test at a non-48 kHz session rate or with an odd channel count.** The tick math is rate-independent by construction (`TICKS_PER_BEAT` ticks per `frame_at` beat), but the `6.5e-15` threshold is rate-dependent and nothing pins the tick schedule at, say, 44 100 Hz.
- **The `#[ignore]`d hardware test cannot catch a wrong conversion** — it only checks `dropped_events() == 0` after 26 pushes, with no loopback. A virtual ALSA loopback (`Midir` has no such harness) or at minimum a `Wire`-level golden-bytes test over a `deviceless` sink for a full bar of a tempo-changed map would.

### Design risks

- **24 PPQN + Start/Stop/Continue without Song Position Pointer cannot re-align a follower.** `Continue` tells gear "resume", not "resume at tick N". After any pause, undo-at-position change, or mid-playback seek the follower is wherever it was. The note defers SPP ("needs 'what is a bar' in the *gear's* terms"), but the alignment story the recorder profile rests on needs either SPP or a periodic re-`Start`; a `Start` on every transport re-entry (not just frame 0) would be a cheap partial mitigation, at the cost of gear that treats `Start` as "go to zero".
- **The tick loop is O(ticks due) with no cap, so an accepted absurd tempo is a CPU denial of service even without the hang.** At 1e9 bpm one 512-frame block owes ~4.27 M ticks; the loop visits every one to count 4.27 M overflows. Bounding the loop by `CLOCK_OUT_CAP` (finding 1's fix) removes both the hang and the DoS.
- **The slot mechanism puts a mutex on the render path to serve a control-side concern.** "Empty the slot across a rebuild" is a control-side decision; encoding it as `Arc<Mutex<Option<Box<dyn MidiSink>>>>` and consulting it per block is what creates the lock and the panic path (finding 6). An `AtomicBool armed` next to a control-only mutex, or a generation counter the node compares, expresses the same invariant with a relaxed load.
- **The decision's soundness rests on the slot being a process-lifetime object.** `rebuild`/`rebuild_prefix` honour that by passing `Some(self.midi_slot.clone())`; `new_at`/`from_script` do not, which is how finding 7 leaks a second connection. A single process-wide slot (or an explicit "shared slot" type distinct from "own slot") would make the invariant structural rather than a convention each constructor has to remember.
- **`RenderMode::Drain` is invisible to the node.** Drain blocks advance the clock and emit ticks. That is harmless only because exports detach; any future offline render that does not detach inherits it (finding 5), and it also means `clock_out` contributes ticks to the graph's "did anything happen" notion of a render.
- **Every session construction re-reads process configuration.** `new_at` is a pure-ish constructor by contract ("**context**: it is fixed here"), but it now opens a device, spawns a thread and reads `std::env::args()` — three side effects in a constructor that tests and the `Default` impl call freely (finding 11).
- **The `dropped_bytes`/`dropped_events` pair is the only realtime-health signal for the wire, and it is invisible** (finding 4). The module header's framing ("a full ring drops the message and counts it … exactly how capture counts an overrun") is only true of the sink, not of the system.

### Checked and clean

- **`crates/media/src/midi.rs` wire conversion** — `to_wire` (108-121): status bytes `0x90`/`0x80`/`0xB0` channel 0 and `0xF8`/`0xFA`/`0xFC`/`0xFB` are all correct; `Trigger` is dropped without counting, as documented and asserted. `note_number` (96-98): `+69`, `round`, `clamp(0,127)` gives MIDI 69 for A4, 0 for pitch −69, 127 for pitch ≥ 58, and a clamped (not wrapped) top note; the 58/59 boundary and exact `.5` rounding are correct by `f32::round`'s half-away-from-zero. `to_7bit` (103-105): 0.0→0, 1.0→127, 0.5→64, out-of-range clamps. `Wire` (66-90) is `Copy`, 4 bytes, `len ∈ {1,3}`, never split across a ring boundary; no MTU or running-status concern (every message carries its own status byte). `MidiOut::open` (167-211): case-insensitive substring matching, loud failure on 0 and on n>1 matches listing what exists, and both `expect`s are provably guarded (`matches.len() == 1`; the name came from `named`).
- **`MidiOut::send` realtime claim** — verified line by line: `to_wire` is pure and returns a stack `Option<Wire>`, `Spsc::try_push` is a single `Acquire` load, a slot write and a `Release` `fetch_add` with no allocation, no lock, no syscall (ring.rs:73-88), and no device. `dropped_events`/`dropped_bytes` accounting is exact — one increment per dropped message, `msg.len` bytes, `Trigger` excluded by design and asserted (midi.rs:377). `RING_MESSAGES = 1 << 10` satisfies `Spsc`'s power-of-two assertion.
- **Writer-thread lifecycle** — spawn failure is an `expect` on a control path (`:239`), the connection is moved into the thread so the port outlives `open`, `Drop` sets the flag and joins (bounded by `WRITER_PARK` = 500 µs in practice), the ring is drained fully before the loop re-checks `stop` (so a normal teardown loses nothing — only the flag-set-before-first-iteration case, finding 12), and `MidiOut::drop` **cannot** run on the render thread: the node, the engine context and the host field each hold an `Arc` clone, and the last one is released by `HostSession`'s drop on the actor thread. `Engine::mount` refuses a duplicate plugin name (render.rs:226), and the actor is single-threaded, so the `Spsc`'s single-producer contract holds across rebuilds.
- **`clock_out.rs` tick math** — `tick_frame` (198-200) and the loop bound (264-274) agree with the tempo map: 48 ticks at frames 0, 1000 … 47000 at 120 bpm/48 kHz, exactly one tick per `frame_at(n/24)`, no double-emit across a block split (the node reads `io.frames`, which graph.rs:1160/1232 sets from the *actual* sub-block length, not `BLOCK`), and tempo changes inside a block stay exact because every frame comes from `frame_at` rather than a carried rate. Zero-length blocks (`frames == 0`) make `block_end == block.frame` and both loops no-op. Tick state is genuinely stateless, so it survives a rebuild with no special case, and the `CLOCK_OUT_CAP` overflow is counted (not grown) on the event scratch. `TransportLog::push`'s `partition_point(f <= frame)` (99-106) inserts after equal frames, so same-frame commands keep log order, and `take_due`'s `partition_point(f < block_end)` is its exact dual — the late-flush-at-offset-0 case is correct and tested.
- **The host slot invariant on every rebuild path** — `replay_to_kind` (2464-2499, seek + undo + redo), `export` (2002-2011), and `replay_full` (2552-2566, test-only) each `detach_midi` before *all* of their renders (rebuild, warm-up run-in, render-to-target, the export clone's rebuild **and** its offline render) and `restore_midi` after, with **no** `?` between the two in any of them — the seek path wraps the fallible work in a closure and restores before the `?` propagates, so a refusal cannot leave the slot empty. There is no nesting: `TransportSeek`/`Undo`/`Export` are actions, so `rebuild` never calls back into a detaching path. `rebuild` and `rebuild_prefix` both pass `Some(self.midi_slot.clone())` (2388-2392, 2577-2581) and `*self = rebuilt` then shares that same `Arc` (2497), so the refill is visible to the new node. `Bounce` is the one gap (finding 5).
- **Purity / byte-identical replay** — mounting `clock_out` produces no audio (`CLOCK_OUT_PORTS = &[]`, `latency() == 0`, no cords), the node only reads the block frame, and the engine-level test pins byte-identical audio with and without a sink (`crates/engine/tests/clock_out.rs:59-72`); the host-level test pins the same across a seek replay (`a_session_without_a_sink_mounts_renders_and_replays_byte_identically`). The declaration is state (`mount clock_out` is `is_state`, so it is in the history and in `session.txt`) while the device is configuration (never logged, `format_command` returns `None` for every transport/media action) — so no refused mutation is logged and no machine-specific fact enters the log.
- **Graph integration** — `NodeIO.frames` is the sub-block length, `out_audio` for a portless node is empty (the node ignores it), and the node is rendered in index order like any other node; `flush_frames()` is unaffected because the node reports zero latency and has no cords. `ClockOutNode` is `Send` (`MidiSink: EventSink: Send`), so nothing new breaks the `!Send`-ness the actor design depends on.
- **`main.rs` flag parsing** — `--midi-out`/`--midi-out=` are skipped (with the value consumed for the spaced form), the first non-flag argument is the script path in either order, and the parser agrees with `midi_out_from_process` on which operand is the port.
- **`live.rs` wiring** — `Snapshot::midi` is a cheap clone-plus-atomic-load in the existing publish pass (593-599), `Default` is filled in, and the audio path is untouched; the render is on the actor thread, not the device callback (the callback only pops the audio ring), so the "render path" the invariant governs is the actor's.

---

## 2. Adversarial verification (verbatim)

I'll verify each claim by reading the actual code.

## Verdicts

### CONFIRMED 5-midi-clockout#1

The fall-through is real and the trigger is a one-line script.

- `crates/engine/src/clock.rs:93-113` — with one open-ended segment, `seg_beats = u64::MAX as f64 * bpm / 60.0 / 48000.0 = 6.4051e12 * bpm`. At `bpm = 1e-15` that is `0.0064051`, which is below `1/24 = 0.0416667`, so `frame_at(1/24)` fails `remaining < seg_beats`, falls out of the loop, and returns `segments.last().start_frame` — i.e. `0`. The `// Unreachable:` comment on line 108 is false. (Verified numerically.)
- `crates/engine/src/plugins/clock_out.rs:208` — `while Self::tick_frame(map, n) < frame { n += 1; }` never terminates for any `frame > 0` (every tick frame is pinned at 0). For `frame == 0` the hang is the main loop at `clock_out.rs:265-274`, whose only exit is `frame >= block_end`.
- Nothing clamps: `crates/host/src/lib.rs:3319` parses the operand as bare `f64`; `crates/host/src/render.rs:499` accepts `1e-15`; `render.rs:802` pushes the segment; `render.rs:1044-1047` hands `&clock.tempo_map` to every node, and `graph.rs:1161,1189` renders every mounted node each block whether or not it is corded.
- Trigger used: `host v1` / `mount clock_out @0` / `set_tempo 0.000000000000001 4` / `transport play` / `bounce 48000 /tmp/x.wav`. `HostSession::render_with_drain` (lib.rs:1874) flushes the scheduled mount, the first block applies `SetTempo` at frame 0, and `ClockOutNode::render` spins on the actor thread forever, `overflows` climbing by 1 per iteration (`clock_out.rs:193`).
- Test evidence holds: `clock_out.rs:539-557` uses `1.0e9`, where `seg_beats ≈ 6.4e21` and the fall-through never triggers; no test anywhere uses a small bpm (`grep` for `SetTempo` in tests: only 96/240/137.5/90/1e6).

One caveat on the *remedy*, not the claim: fix (a) as written ("stop after `n - n0 >= CLOCK_OUT_CAP` and `fetch_add` the remainder") cannot produce an exact remainder without iterating the due ticks, which is exactly what the existing test asserts (`emitted + overflows == due`, clock_out.rs:554). A count derived from the tempo map, or a separate `due_count`, is needed to keep that assertion true.

### CONFIRMED 5-midi-clockout#2

- `crates/host/src/lib.rs:440-457` — `is_state` omits `TransportPlay`/`TransportStop`; `process` only calls `commit_state` for state commands (`lib.rs:2732-2740`), so a play never enters `history` and `rebuild` (lib.rs:2393-2415) never re-applies it.
- `crates/host/src/lib.rs:680` — every `new_at_with` constructs a brand-new `TransportLog`; `carry_over` (lib.rs:2524-2546) copies `midi_slot`, `midi_port`, `redo`, `playing` — not `transport`. `grep` for `transport.push` in the workspace returns exactly two sites, both in `apply` (lib.rs:1426, 1432): nothing re-feeds a rebuilt log.
- `lib.rs:2417` `rebuilt.playing = self.playing`, so the pump (live.rs:425) keeps rendering and the node emits ticks from the new position with no transport event.
- Trigger used: mount `clock_out`, `TransportPlay`, `render(48_000)`, then `TransportSeek { frame: 24_000 }`. The shipped test at lib.rs:7640-7659 asserts `transport_events(&sends) == vec![(0, "Start")]` immediately after the seek and then only checks that resumed tick *frames* are `>= 24_000` — it pins the absence and cannot fail. The note's sentences (host note lines 29-30 and 34) are exactly as quoted: the tap is never re-fed by a replay, and a mid-playback seek has no next `play`.
- Two corrections to the reviewer's arithmetic: after the seek the session is at tick **24** (frame 24 000 = beat 1 at 120 bpm), not 48; and a follower is ahead by 24 pulses (one beat = 0.5 s) — so "half a second" is right, "the session is at 48" is not. Also, "nothing can ever correct it" overstates: a user stop/play would re-anchor, but the host never sends one on its own. Verdict stands on the code fact and the automatic-path consequence.

### CONFIRMED 5-midi-clockout#3

- `crates/engine/src/plugins/clock_out.rs:112-120` — `out.extend(queue.drain(..at))` with no bound; `out` is `transport_scratch`, `Vec::with_capacity(CLOCK_OUT_CAP)` (line 177), cleared at line 233, so 65+ due entries reallocate inside `ClockOutNode::render` (called at line 242). `Vec::extend` over `Drain` (`TrustedLen`) calls `reserve`, so the growth is real.
- The guard exists only on the sibling: `clock_out.rs:189-195` `push` checks `self.scratch.len() < self.scratch.capacity()`. Nothing bounds `TransportLog::push` (clock_out.rs:99-106) — it is an unbounded `Vec::insert`.
- Trigger used: `TransportPlay`/`Stop` have `at_frame() == None` (lib.rs:412-426) so `process` pre-renders nothing (lib.rs:2716-2723); 70 such lines after `mount clock_out @0` followed by a `bounce` leaves 70 entries with frame 0 in the log, and the first block drains all 70. The live variant is constructible too: live.rs:425 renders only `if session.is_playing()`.
- The doc mismatch is real: `take_due`'s own doc (line 111) says "The node reuses a preallocated buffer, so draining allocates nothing". (The reviewer's quote "the render path never grows it (see `CLOCK_OUT_CAP`)" is the `scratch` field doc at line 142-143, not `transport_scratch`'s, which says only "also preallocated, for the same reason" — the take_due quote carries the claim anyway.)
- Test evidence holds: `crates/engine/tests/spike_a.rs:834-864` registers and mounts `clock_out` but the `engine()` fixture provides neither a sink nor `transport` under the context, so `take_due` is never entered; the module's longest transport test queues 4 entries (clock_out.rs:513-517).
- Severity is arguable (one bounded realloc, not unbounded growth), but the code does what the claim says.

### CONFIRMED 5-midi-clockout#4

- `crates/media/src/midi.rs:249-250` — "the loud bound, read by the host (a nonzero count means the writer stalled)".
- `crates/host/src/lib.rs:2271-2277` — `Box::new(out) as Box<dyn MidiSink>` erases the accessors at the only construction site. `MidiOutStatus` (lib.rs:488-496) carries `port` + `overflows`, and `midi_status` (lib.rs:2168-2178) reads only `CLOCK_OUT_OVERFLOWS_KEY` — the *plugin's* cap counter, not ring drops. `main.rs:141-146` prints the same two fields; `live.rs:598` publishes the same `midi_status`.
- `grep -rn "dropped_events\|dropped_bytes" crates` returns hits only inside `midi.rs` (definition, the `a_full_ring_drops_and_counts` test, and the `#[ignore]`d `midi_out_real_port_sends_a_few_ticks`). `ports()` is likewise only called from that ignored test. So a stalled writer drops clock with nothing in `Snapshot` or stdout reporting it.
- The trigger is real and the consequence follows. This is a doc/code defect, not a wrong number.

### CONFIRMED 5-midi-clockout#5

- `crates/host/src/lib.rs:1684-1688` — the `Bounce` arm calls `self.render_with_drain(*frames)?` with no `detach_midi`, against `export` at lib.rs:2007-2010 which does `let device = self.detach_midi(); … self.restore_midi(device);`. `grep -n "detach_midi"` gives exactly three call sites: 2007 (export), 2483 (`replay_to_kind`), 2559 (`replay_full`) — never `Bounce`.
- The sub-claims check out: `ClockOutNode::render` (clock_out.rs:223-291) reads only `block.frame` and `block.tempo`, never `block.mode`; `Engine::drain` advances the clock (render.rs:1052) and passes `RenderMode::Drain` blocks through `graph.render_drain` (render.rs:1044-1050), so the drain tail emits ticks; `render_with_drain_aligned` (render.rs:988-1006) renders `frames + flush_frames()` and trims the head from the **audio** only, so the transit frames still generate ticks that lead the file.
- The wire effect: whatever is in the transport log (e.g. a `Start` from an earlier `transport play`) plus a tick stream for the whole bounce and its drain, and no `Stop`.
- No test covers it: `an_export_clone_sends_nothing` (lib.rs:7665-7722) is the only offline-render-with-sink test and it exercises `Export` only; the module's other four tests (7528-7659) never bounce.
- The note (host note, "Let an export drive gear", lines 47-49) states the principle and only mentions export. Confirmed as an undeclared divergence, not as a contradiction of an explicit `Bounce` rule.

### PARTLY 5-midi-clockout#6

Right about the code, wrong about the invariant and the consequence.

- The code fact is exact: `clock_out.rs:241-242` → `TransportLog::take_due` → `.lock().expect("the transport log is not poisoned")` (clock_out.rs:115-116), and `clock_out.rs:282-288` → `self.sink.lock().expect("the midi.out sink is not poisoned")`. Two `std::sync::Mutex` acquisitions with `.expect` panic paths, inside `ClockOutNode::render`.
- But "invariant 1 says lock-free and no panic paths" is false. `docs/theory-of-the-program.md:133-140` defines **I1** as "Every mutation is logged at call time with its absolute frame…", and **I2** as the purity/determinism invariant. Neither mentions locks or panics. The real performance rule is `docs/architecture-explainer.md:317-320`: "The steady-state render loop never allocates and never blocks."
- Against that rule, the sink mutex is a **deliberate, recorded decision**, not a violation: `.agents/notes/implemented/architecture/2026-09-29-midi-clock-out-in-the-engine.md:44-46` — "**One `sink.send(&scratch, block.frame)` per block**, with the mutex held only across that call. A single sender means the lock is uncontended…". The `SharedMidiSink` doc (clock_out.rs:53-59) says the same. Only the `TransportLog` mutex is undocumented as a render-path cost.
- The panic paths are effectively unreachable. Poisoning requires a panic while holding: the only other holders are `detach_midi`/`restore_midi` (lib.rs:2505-2519 — a `take` and an assignment, neither of which can panic) and `TransportLog::push` (a `Vec::insert`, which panics only on OOM). So "a panic here is on the thread that renders audio" describes a path no code in the repo can open.
- Contention latency is real but bounded and unconstructible in practice: the control side never holds either lock across a render, and the render side is the single-threaded actor that *is* the control side, so the only preemption window is a concurrent `HostHandle` command. The reviewer's own verification paragraph concedes "not deadlock".
- What survives as a genuine, smaller finding: `MidiOut::send`'s doc (midi.rs:275-277) claims "No lock, no allocation, no device" at a layer where it is true, while the node wrapping it is neither, and nothing measures it.

### New findings

**(a) The same unbounded loop is reachable at the *other* end of the tempo range, with an easier trigger than #1.** `crates/engine/src/plugins/clock_out.rs:265-274` iterates once per due tick; `CLOCK_OUT_CAP` bounds memory (line 189-195) but not the iteration count, and the only exit is `frame >= block_end`.

```rust
let mut n = Self::first_tick_at_or_after(block.tempo, block.frame);
loop {
    let frame = Self::tick_frame(block.tempo, n);
    if frame >= block_end { break; }
    self.push(ExternalEvent::Clock { offset: (frame - block.frame) as u32 });
    n += 1;
}
```

Ticks due in a 512-frame block ≈ `512/48000 × 0.4 × bpm` = `0.00427 × bpm`. Trigger: `set_tempo 1000000000000 4` (1e12, an ordinary 13-digit literal, no f64 edge) + `clock_out` + one render → ~4.3e9 iterations in a single block; at 1e15 it is ~4.3e12, i.e. an effective hang with no fall-through needed. Consequence: the actor thread stops servicing commands and the live pump's audio ring underruns. The existing test at 1e9 bpm (clock_out.rs:539-557) is 4.3e6 iterations per block — fast enough to pass, which is exactly why it cannot see this. A fix for #1 must bound this loop too.

**(b) The export rebuild collapses every `set_tempo` to frame 0, so an export of a session with a mid-piece tempo change renders beat-synced generators at the wrong tempo.** `crates/host/src/lib.rs:2407-2412` applies the clone's history with `cmd.at_now()`, and `at_now` (lib.rs:349-350, 377-385) sets `at_frame: None`, so `engine.set_tempo` stamps the segment at the clone's current frame — 0 for every one of them:

```rust
None => {
    // `at_now`: order decides, not timeline position, so applying the
    // state does not render the clock through the piece.
    for cmd in entry { rebuilt.process(&cmd.at_now())?; }
}
```

With `set_tempo 120 4 @0` and `set_tempo 60 4 @96000` in the history, the clone's map holds two segments both at frame 0; `tempo_at`/`beat_at`/`frame_at` then report 60 bpm for the whole file, while the live session plays 120 for the first 96 000 frames. Consequence: `export` (lib.rs:2002-2011) writes bars that do not line up with the live session's whenever a beat-domain node is mounted (`euclidean`→`scale`/`tone`, graph.rs:494); clips-only graphs are unaffected because clip positions are frame-based. The sibling rebuild path calls tempo "**the exception to `at_now`**" and places each segment at its own frame (lib.rs:2589-2605) — so the two rebuild paths disagree about the same command, and no test exercises an export of a session that has a tempo change (`grep` for `SetTempo` in `crates/host/tests/` finds only reference_host.rs:199 and the overflow test at lib.rs:7600). The `rebuild` doc ("every state command is applied at the present instant") arguably sanctions it for *state*; tempo is frame-placed value state, which is why `rebuild_prefix` excludes it.

**(c) A `load` opens a second MIDI port while the current one is still open, instead of carrying the slot the way every other rebuild does.** `crates/host/src/lib.rs:2315-2326` (`from_script`) → `new_at` (lib.rs:620-623) → `midi_out_from_process()` (lib.rs:2257-2284) → `MidiOut::open`, a fresh `midir` connection plus a fresh writer thread. Both load paths build the new session *before* dropping the old one: `load_session` (lib.rs:1269-1276, `*self = loaded` last) and the live actor's `Request::Load` (live.rs:399-407, `session = fresh` after the match). By contrast, every in-session rebuild passes the existing slot: `rebuild` (lib.rs:2388-2392) and `rebuild_prefix` (lib.rs:2577-2581) both use `new_at_with(rate, Some(self.midi_slot.clone()), …)`. Consequence I can verify from the code: two devices and two writer threads exist simultaneously for the duration of the construction, and the old connection is closed only when the old session drops. Whether that fails or merely overlaps depends on whether the ALSA port/driver is exclusive for a second client, which I cannot test here (no device, and running the host is out of scope) — but if the second `connect` fails, `midi_out_from_process` takes the `Err` arm (lib.rs:2278-2282) and the loaded session runs **silently with no clock-out at all**, reporting only on stderr. A one-line fix is to have `from_script`/`load_session` adopt the live session's slot the way `rebuild` does.
