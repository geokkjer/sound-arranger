# Space-bunny external review — mechanical bug sweep across the workspace

> **Disposition.** External deep review, 2026-09-29, against `49e8774` (clean tree). Reviewer:
> Space Bunny (`opencode/space-bunny-free`) driven through OpenCode by DeepSeek Harness; the
> supervisor (DeepSeek-V4.1-Flash) wrote the scope, then handed every CRITICAL/MAJOR claim to an
> **independent adversarial verifier** that had to falsify it before it counted.
>
> This file is the **mechanical bug sweep across the workspace** scope, verbatim: the reviewer's report, then its verifier's
> verdicts. What was done about each finding — fixed, deferred or refuted — is in
> [the index](2026-09-29-space-bunny-review.md). Findings are the reviewer's, not the
> supervisor's: read the verdicts before acting on any single claim.

---

## 1. Reviewer's report (verbatim)

## Mechanical bug sweep review

Scope: every Rust source file in `crates/engine`, `crates/host`, `crates/media`, `crates/workflow`, `spikes/tui-shell`, `spikes/iced-shell` (42,118 lines, 66 files), at `49e8774`. Method: mechanical candidate enumeration with `#[cfg(test)]` modules stripped, then each enclosing function, its callers and its tests read by hand. Baseline assumed: 326 tests green, clippy clean.

### CRITICAL

1. **A `mount`/`unmount` scheduled at a frame inside a render block is applied *on* the render stack — allocating ~32 KiB per node and freeing it again on retire**

- **Severity**: CRITICAL (invariant 1: "the audio render path is allocation-free")
- **Location**: `crates/engine/src/render.rs:1148-1161` (the gate), `crates/engine/src/render.rs:784` / `475-492` (what it lets through), `crates/engine/src/graph.rs:870-871` (what it allocates)
- **Trigger**: any `mount`/`unmount` whose `at_frame` the clock has already reached when the *next* `render` starts. Concretely, from the text format: `host v1` containing `mount euclidean steps=8 @500` followed by `bounce 2000 /tmp/o.wav`. `HostSession::process` renders up to frame 500 and *then* applies the mount (`crates/host/src/lib.rs:2716-2724`), so the `SchedEvent::Mount` sits in the queue at frame 500 with `clock.frame() == 500`. The next `render` enters `render_block` with `pos = 500`, `f1 = 500 + 512`; `peek_frame()` is 500 `< f1`, so `next == pos` and the inner loop pops and applies it.
- **Wrong behaviour**: `apply_event → apply_mount → plugin.apply → graph.add_node` runs inside `render_block`. `add_node` pushes ~10 `Vec`s and one `RingDelay::with_capacity(MAX_PDC * MAX_PDC_CHANNELS)` = 8192 `f32` = **32 KiB**, plus two `Box`es, a `factory(params)` allocation, and three `HashMap` inserts. The mirror case (`unmount euclidean @500`) runs `apply_unmount` → `graph.remove_node`, which **frees** that 32 KiB plus 10 `Vec` headers on the same stack — the exact class the previous review's critical #2 was fixed for on the player side, and not fixed for the engine's own nodes. In the live host this stack is `live::fill_audio` → `session.render`, i.e. the thread that must keep the output ring half-full, so a stall is a device underrun; in the offline path it is a `Vec` realloc in the middle of a block.
- **Evidence**:
  ```rust
  // crates/engine/src/render.rs:1148
                          if self.changes_master_width(&event) {
                              // ... PARK it ...
                              self.parked.push(event);
                              continue;
                          }
                          self.apply_event(event);
  ```
  ```rust
  // crates/engine/src/render.rs:927-931 — the only two events the gate parks
          match (event, declared) {
              (SchedEvent::Mount { .. }, Some(ch)) => ch != self.graph.out_channels().max(1),
              // A plugin with no audio Out cannot become the bus.
              _ => false,
          }
  ```
  `euclidean` declares only a `Trigger` out-port, so `declared == None` → `_ => false` → applied on the render stack. Same for `clock_out` (no ports) and for a mono `tone` when no mixer is mounted (`out_channels() == 1`, so `1 != 1` is false).
  ```rust
  // crates/engine/src/graph.rs:870
          self.delays
              .push(RingDelay::with_capacity(MAX_PDC * MAX_PDC_CHANNELS));
  ```
- **Fix**: park *every* `SchedEvent` that is not a value-only, allocation-free event — i.e. park `Mount` and `Unmount` unconditionally, exactly as `Arrangement` is parked, and let `flush_scheduled` (already called at every render-call boundary by `channels_for_render`) apply them. This also removes the need for `changes_master_width` entirely, since the width is then constant per call by construction. A cheaper interim fix: extend `changes_master_width` to return `true` for any `Mount`/`Unmount` that is not a pure width no-op.
- **Verification**: both counting-allocator tests are steady-state only — `crates/engine/tests/spike_a.rs:851` primes with `let _prime = e.render(512);` before arming the counter, and line 848 schedules the unmount at frame **100 000**, deliberately outside the 8192-sample measured region. `crates/engine/tests/parked_arrangements.rs` covers only the `Arrangement` arm. No test arms the allocator across a scheduled mount/unmount. A test that measures `render_into` with one event scheduled inside the first block would fail today.

### MAJOR

2. **`SetClipFade` validates `fade_in + fade_out` with an unchecked `u64` addition — a panic in debug, a logged invalid op in release**

- **Severity**: MAJOR (a user-reachable panic in debug; invariant 3 in release — the op is applied *and logged* with fades longer than the clip)
- **Location**: `crates/media/src/timeline.rs:759`
- **Trigger**: `arrange set_clip_fade t0 c0 18446744073709551615 1` in a `host v1` script. `parse_arrange`'s `u(i)` helper (`crates/host/src/lib.rs:3678-3688`) has no bound, and the arm (`crates/host/src/lib.rs:3900-3908`) passes both operands straight through.
- **Wrong behaviour**: `u64::MAX + 1` wraps to 0, so `0 > src_len` is false and the op is **accepted and written to the session log**. The clip now has `fade_in = u64::MAX`, which `ArrangerNode::new` rejects in `validate_clip` (`timeline.rs:349`) → `wire_arranger` fails → `render()` returns `Err`, so **every subsequent bounce, export, seek and undo fails** with `arrange: clip 'c0' fades exceed the clip length`. The value also silences the clip before that: `fade_gain` computes `(off / u64::MAX as f32).min(1.0) == 0.0`, so `gin.min(gout) == 0`. `save` writes the poisoned state happily (it round-trips as text), so the dead end persists across a reload. In a **debug** build the same input panics on the overflow check *while `ClipEditor::apply` holds the timeline mutex* (`clip_editor.rs:550-556`), poisoning it so that `arrangement()` returns `Err("timeline poisoned")` for the rest of the session.
- **Evidence**:
  ```rust
  // crates/media/src/timeline.rs:759
                  if fade_in + fade_out > self.tracks[ti].clips[ci].src_len {
                      return Err("fades exceed the clip length".into());
                  }
                  self.tracks[ti].clips[ci].fade_in = *fade_in;
                  self.tracks[ti].clips[ci].fade_out = *fade_out;
  ```
  The crate's own test comment asserts the opposite of what ships:
  > `// Gated on debug_assertions because the mechanism is an overflow check (release builds have no reachable poison path through the public API).` — `crates/host/src/lib.rs:4055-4057`
  
  The mechanism is a plain `u64 +`, so the release path is reachable through `parse_script`, and it does not panic — it corrupts the value.
- **Fix**: `fade_in.checked_add(fade_out).is_some_and(|sum| sum <= src_len)`, i.e. refuse on overflow instead of wrapping; the same unchecked addition exists in `validate_clip` at `timeline.rs:349` and needs the same treatment. Then update the test at `crates/host/src/lib.rs:4058` to drive the *refusal* rather than the poison.
- **Verification**: `crates/host/tests/arranger_commands.rs:570` covers only `set_clip_fade t0 c0 64 128`; `crates/workflow/src/lib.rs:770` only `4800 4800`. No test uses a fade sum near `u64::MAX`.

3. **`PlaybackNode::pending` reallocates on the render path past 8 queued splices — reachable from a script**

- **Severity**: MAJOR (invariant 1; this is the previous review's minor, still open, but now with a concrete user-reachable trigger)
- **Location**: `crates/media/src/stream.rs:322` and `355-360`
- **Trigger**: a `host v1` script with nine `splice` lines before a `bounce`. `HostCommand::Splice` pushes each into the **unbounded** mailbox `VecDeque<SpliceCmd>` (`crates/host/src/lib.rs:1526`); the first `PlaybackNode::render` call in the bounce runs `drain_mailbox`, which moves all nine into a `VecDeque::with_capacity(8)` — the 9th `push_back` reallocates (and copies eight `SpliceCmd`s, each owning a `FilePlayer`) **inside `render_into`**.
- **Wrong behaviour**: an allocation plus an eight-element move on the render stack at the start of a bounce; in the live path a ring underrun. The code even says so, and ships it: *"pushes beyond the preallocated capacity would allocate on the render path (a documented Phase-0 edge …)"*.
- **Evidence**:
  ```rust
  // crates/media/src/stream.rs:318
              // Bounded command buffer: pushes beyond the preallocated capacity
              // would allocate on the render path (a documented Phase-0 edge —
              // the profile schedules splices on clean boundaries, so bursts
              // beyond 8 are not expected).
              pending: VecDeque::with_capacity(8),
  ```
  ```rust
  // crates/media/src/stream.rs:355
      fn drain_mailbox(&mut self) {
          if let Ok(mut mbox) = self.mailbox.try_lock() {
              while let Some(cmd) = mbox.pop_front() {
                  self.pending.push_back(cmd);
              }
          }
      }
  ```
- **Fix**: bound the mailbox with the same limit the host enforces on splices, and `try_reserve` on the control side; or size `pending` from `mailbox.len()` at the first drain (a one-time allocation at the render call's first block) instead of a fixed 8. The clean answer is the mailbox bound, since `Splice` is a control-side command with no reason to be unbounded.
- **Verification**: `crates/media/tests/spike_b.rs` splices once or twice per case; nothing queues nine.

4. **`WavWriter::recover` derives the sample width from `bits == 32`, so recovering a 24-bit PCM file truncates it to two-thirds of its length**

- **Severity**: MAJOR (silent, irreversible destruction of a third of the audio)
- **Location**: `crates/media/src/wav.rs:409`
- **Trigger**: a 24-bit PCM WAV in a pool directory whose declared `data` size does not match the file length — exactly the condition `Pool::recover` exists for (`pool.rs:252`, `if !src.finalized`). 24-bit is a first-class supported format: `parse_header` accepts it (`wav.rs:119`), `WavReader` decodes it, `Pool::list` indexes it, and `Pool::conform` deliberately leaves a 24-bit mono source at the session rate untouched (`pool.rs:690`). A half-copied 24-bit file is the ordinary way to get there.
- **Wrong behaviour**: `frames` is computed correctly (`actual_bytes / block_align` with `bits/8 == 3`), but `data_bytes(frames, channels, h.bits == 32)` is called with `float = false`, so `bytes_per_sample` is 2 instead of 3. `f.set_len(h.data_offset + data_bytes)` then cuts the file to 2/3 of its audio, and the header is patched to declare that shorter length, so the truncation becomes permanent and looks well-formed on the next read. `patch_sizes` (line 488) is not affected — it takes the writer's own `float` flag — which is why the asymmetry survived.
- **Evidence**:
  ```rust
  // crates/media/src/wav.rs:407-409
          // size field rather than writing a corrupt small header (>4 GiB).
          let data_bytes = data_bytes(frames, h.channels, h.bits == 32)?;
  ```
  ```rust
  // crates/media/src/wav.rs:505-509
  fn data_bytes(frames: u64, channels: u16, float: bool) -> Result<u64, String> {
      let bytes_per_sample: u64 = if float { 4 } else { 2 };
  ```
  `24-bit ⇒ float == false ⇒ 2 bytes/sample`.
- **Fix**: pass the width, not a float flag: `data_bytes(frames, channels, (h.bits / 8) as u64)`, and change the signature to take `bytes_per_sample: u64` (validated to 2/3/4) so the three call sites cannot drift apart again.
- **Verification**: `crates/media/tests/pool.rs:74,97` recover only 16-bit and float takes; `wav.rs`'s 24-bit test (`a_twenty_four_bit_wav_reads_back`, line 821) never calls `recover`. **Caveat, stated honestly**: nothing in the shipped code calls `Pool::recover` — only `crates/media/tests/pool.rs` does (`rg '\brecover\('` finds no call in `crates/host` or the shells) — so today this is reachable through the public, documented crash-recovery API and its own tests, not through the product. See *Test gaps*.

5. **`Pool::import` deletes the previous material before the rename that replaces it commits**

- **Severity**: MAJOR (data loss of the user's only copy, and a direct contradiction of the function's own documented contract)
- **Location**: `crates/media/src/pool.rs:486-494` (mono path) and `423-436` (multi-channel path)
- **Trigger**: import a file whose id is already in the pool, on a filesystem where the commit `fs::rename` fails — ENOSPC after the temp file was written, a read-only or full pool directory on the *destination* metadata, or Windows' refusal to replace an existing file. `replace_sources` runs at line 486, the rename at 487.
- **Wrong behaviour**: `{id}.wav` and `{id}.peaks` are deleted at line 486; the rename then fails, the temp is removed, and `import` returns `Err` with the pool **emptier than before the call** — the previous take is gone and nothing replaced it. The contract the function states is the opposite:
  > *"Called only once the new material is safely written, so a failed import changes nothing."* — `pool.rs:573-575`
- **Evidence**:
  ```rust
  // crates/media/src/pool.rs:486
          self.replace_sources(&id, 1);
          if let Err(e) = fs::rename(&tmp, &dest) {
              let _ = fs::remove_file(&tmp);
              return Err(format!(
                  "import {}: commit {}: {e}",
  ```
  `replace_sources` ends in `let _ = fs::remove_file(&path); let _ = fs::remove_file(path.with_extension("peaks"));` (`pool.rs:598-599`) — errors swallowed, so a permission problem there is invisible. The multi-channel path has the same ordering at line 423 and additionally leaks `.converting` temporaries when `rebuild_peaks` fails mid-loop at line 440.
- **Fix**: move `replace_sources` to *after* the successful commit (the commit is a same-directory rename, which is atomic; the stale siblings are only reachable by id, so deleting them one step later is safe), or stage the removals and only issue them once the destination is in place. Return the `remove_file` errors instead of discarding them.
- **Verification**: `crates/media/tests/pool.rs` and `p1_2.rs` import over existing ids only on the success path; nothing injects a commit failure.

6. **Mount parameters are checked for finiteness only — a single `mount` line can allocate gigabytes or stall the render path for minutes**

- **Severity**: MAJOR (unbounded allocation; render-path denial of service; the declared parameter ranges exist but are not applied on the mount path)
- **Location**: `crates/engine/src/plugins/euclidean.rs:23-25` and `107-119`; `crates/engine/src/plugins/tone.rs:76-88`; the only mount-side check is `crates/engine/src/render.rs:231-235`
- **Trigger**: any of these lines in a `host v1` script (all params are in `HOST_PARAMS`, so all pass `in_list`):
  - `mount euclidean steps=4000000000 @0` → `euclid` allocates `vec![false; 4_000_000_000]` (4 GiB) and `apply` then `pattern.clone()`s it (8 GiB) → allocation failure aborts the process. `mount euclidean steps=-1` gives `n = 1`, harmless.
  - `mount euclidean pulses_per_beat=4000000000 @0` → `step_beats = 2.5e-10`; `EuclideanGen::render` (`graph.rs:480-497`) computes `s1 - s0 ≈ 0.0213 beats / 2.5e-10 ≈ 8.5e7` and loops that many times **per 512-frame block**, each iteration calling `TempoMap::frame_at`. One second of audio becomes minutes of render.
  - `mount tone blip_len=4000000000 @0` → `TONE_PARAMS` declares `blip_len ∈ [1, 1_000_000]` and `set_param` enforces it, but `tone_factory` truncates the f32 to `u32` unchecked. `ToneGen::has_tail()` (`graph.rs:607-609`) then reports a tail for 4 × 10⁹ samples ≈ 23 hours, so **every** `bounce` and `export` hits `MAX_DRAIN_FRAMES`, sets `DrainOutcome.capped`, and fails with *"bounce drain hit the 48000-frame bound"*. The session can be loaded, edited and saved but never rendered.
- **Wrong behaviour**: an OOM abort, a multi-minute render stall, or a permanently unrenderable session, all from one well-formed script line whose parameter the plugin's own `ParamDef` says is out of range.
- **Evidence**:
  ```rust
  // crates/engine/src/plugins/euclidean.rs:22-25
  pub fn euclid(steps: u32, pulses: u32, rotation: u32) -> Vec<bool> {
      let n = steps.max(1) as usize;
      let k = pulses.min(steps) as usize;
      let mut pattern = vec![false; n];
  ```
  ```rust
  // crates/engine/src/render.rs:229-235 — the whole of mount-param validation
          // Mount params get a finiteness check (GLM-5.3 #9): `set_param` has one,
          // but a `mount ... NaN` would otherwise reach the plugin's apply silently.
          for (pname, v) in params {
              if !v.is_finite() {
                  return Err(format!("mount param '{pname}' must be finite, got {v}"));
              }
          }
  ```
  ```rust
  // crates/engine/src/plugins/tone.rs:26-36 vs 86 — the declared range vs the cast
      ParamDef { name: "blip_len", min: 1.0, max: 1_000_000.0 },
  ...
      blip_len: get("blip_len", 1200.0) as u32,
  ```
- **Fix**: give each plugin a per-param range for its *mount* params too (the `ParamDef` table already exists — add the mount-only entries with the mount-only ranges) and validate in `validate_mount` against it, so `steps`, `pulses_per_beat`, `blip_len`, `note_len`, `gain`, `blip_len` and `count` are all bounded before the factory runs. `MIXER_CHANNELS_SANITY` is the model to follow, and its comment (*"It exists so `channels=1000000` cannot allocate gigabytes"*) is exactly the argument the other plugins never got.
- **Verification**: `mixer_factory` has the bound and `crates/engine/tests/phase1_mixer.rs` tests it; no test mounts any other plugin with an out-of-range param. This is the previous review's major #9, half-fixed (finiteness only, no ranges).

7. **The `clock_out` render path acquires two mutexes and calls a device sink while holding one, against a lock-free invariant**

- **Severity**: MAJOR (invariant 1: "lock-free")
- **Location**: `crates/engine/src/plugins/clock_out.rs:112-120` and `282-290`; the host's counterparts at `crates/host/src/lib.rs:2505-2519`
- **Trigger**: mount `clock_out` and render. Every block takes `TransportLog::queue.lock()` and, if the block produced any event, `self.sink.lock()` — and calls `device.send(&self.scratch, block.frame)` **inside** that second lock.
- **Wrong behaviour**: a lock acquisition on the render path, twice per block, each with an `.expect()` that is a panic path. Both are contended in principle: `HostSession::detach_midi`/`restore_midi` take the same sink slot from the control side around every rebuild and every export, so a control-side refill can block a render (and vice versa). Worse, the lock is poisonable: any panic inside a `MidiSink::send` implementation poisons the slot, after which `ClockOutNode::render`'s `.expect("the midi.out sink is not poisoned")` panics **on every subsequent block** — a device-sink bug becomes a permanent render-path panic. `MidiOut::send` (the shipped implementation) only pushes to a lock-free ring, so today's blast radius is one uncontended CAS; the defect is that the trait boundary permits far worse.
- **Evidence**:
  ```rust
  // crates/engine/src/plugins/clock_out.rs:282
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
  ```rust
  // crates/engine/src/plugins/clock_out.rs:112-119
      pub fn take_due(&self, block_end: u64, out: &mut Vec<(u64, Transport)>) {
          let mut queue = self
              .queue
              .lock()
              .expect("the transport log is not poisoned");
  ```
  The module doc claims the opposite: *"The lock is held only for the call, and with a single sender it is uncontended"* (line 277) — the transport log's writer is the host's control path, not the same thread, once a shell is allowed to push transport itself.
- **Fix**: replace the `Mutex<Option<Box<dyn MidiSink>>>` slot with a `RwLock` read for the render path (or, better, an `arc_swap`-style atomic pointer to the sink so the render path does a single acquire load and no lock at all), drop the second `Mutex` by making the transport log an SPSC ring like every other control→render handoff in the codebase, and replace both `.expect()`s with a counted-and-reported poison.
- **Verification**: `crates/engine/tests/clock_out.rs` asserts tick content and the overflow counter, never that the render path is lock-free; the no-alloc test (`spike_a.rs:835`) *does* mount `clock_out`, but a `Mutex` lock is not an allocation, so it cannot see this.

### MINOR

8. **The compressor's peak envelope decays into subnormal range and stays there for seconds after a loud passage**

- **Severity**: MINOR (performance, not correctness — a real-time hazard on any target without FTZ/DAZ)
- **Location**: `crates/engine/src/plugins/master.rs:397-402`
- **Trigger**: mount `master` (the default `-12 dB` threshold / `120 ms` release) and let the mix fall silent after a loud passage.
- **Wrong behaviour**: `env = release_coeff * env + (1 - release_coeff) * level` with `level == 0` gives `env *= exp(-1/(0.12 × 48000)) ≈ 0.99982` per sample. From 1.0, `env` crosses into the subnormal band (below 1.18e-38) after ≈ 505 000 samples ≈ 10.5 s and stays subnormal for roughly another 170 000 samples ≈ 3.6 s before flushing to zero. Every sample in that window performs a subnormal multiply-add — typically 10-100× slower on x86 without a flush-to-zero mode, on the render path, in the one node that every session has mounted. The limiter's `self.gain` release and the two delay lines have the same shape.
- **Evidence**:
  ```rust
  // crates/engine/src/plugins/master.rs:397-402
              let coeff = if level > self.env {
                  self.attack_coeff
              } else {
                  self.release_coeff
              };
              self.env = coeff * self.env + (1.0 - coeff) * level;
  ```
- **Fix**: floor the envelope once per block, not per sample — e.g. `if self.env < 1e-20 { self.env = 0.0; }` outside the sample loop, which is free and keeps the decay inaudible (1e-20 is 400 dB down). The same guard belongs on `self.gain`.
- **Verification**: `crates/engine/tests/phase1_mixer.rs` and `master.rs`'s own tests render blocks of a few thousand frames — far short of the 10 s needed to reach the subnormal band. Nothing here is audible, so no content assertion could catch it; it needs a timing or a bit-pattern assertion.

9. **The render path's cumulative-latency sum wraps where its control-side twin saturates**

- **Severity**: MINOR (inconsistency; needs a pathological plugin)
- **Location**: `crates/engine/src/graph.rs:1185` vs `crates/engine/src/graph.rs:1100-1102`
- **Trigger**: a plugin whose `AudioNode::latency()` returns more than `u32::MAX - 4096`. No shipped plugin does (the master's is clamped to `MAX_PDC/2` at `master.rs:271`), so this is a latent trap for the next opaque node, not a live bug.
- **Wrong behaviour**: the two copies of the same computation disagree, and the render-path copy is the one that decides the PDC delay:
  ```rust
  // crates/engine/src/graph.rs:1185 — render path
              self.cum[i] = (base + self.nodes[i].latency()).min(MAX_PDC as u32);
  ```
  ```rust
  // crates/engine/src/graph.rs:1100 — control path (cumulative_latency)
              cum[i] = base
                  .saturating_add(self.nodes[i].latency())
                  .min(MAX_PDC as u32);
  ```
  In release a wrapped sum can land *below* `MAX_PDC` after the `.min`, so the node is under-compensated with no error; `flush_frames()` (which uses the saturating copy) would then compute a different transit from the delay the render actually applied, and `render_with_drain_aligned` would trim the wrong number of head frames.
- **Fix**: use `saturating_add` at `graph.rs:1185` — it costs nothing on `x86-64` and makes the two copies identical by construction.
- **Verification**: no test uses a `latency()` above `MAX_PDC`; `crates/engine/tests/spike_a.rs:645` uses 3.

10. **`pool_sources()` maps every read failure to `None`, discarding `PoolIndex::errors`**

- **Severity**: MINOR (swallowed error; loses the diagnostic the pool exists to produce)
- **Location**: `crates/host/src/lib.rs:764-768`
- **Trigger**: any pool whose directory cannot be read, or that contains one unreadable `.wav` (the `errors` case).
- **Wrong behaviour**: a shell polling `HostOutcome.pool_sources` cannot tell "no pool set" from "the pool is unreadable" or "one of your takes is corrupt" — it gets `None` and renders an empty panel. The pool module goes out of its way to *report* rather than abort:
  > *"A source that cannot be read is reported in `errors` and skipped — not fatal to the whole index."* — `pool.rs:55-56`
  
  The host then throws that report away, so the source of the problem is unreachable from the shell.
- **Evidence**:
  ```rust
  // crates/host/src/lib.rs:764
      pub fn pool_sources(&self) -> Option<Vec<media::PoolSource>> {
          let dir = self.pool_dir.as_ref()?;
          let pool = media::Pool::open(dir).ok()?;
          Some(pool.list().ok()?.sources)
      }
  ```
- **Fix**: return `Result<Vec<PoolSource>, String>` carrying `index.errors` (or add a `pool_errors()` accessor the shell can show), so a shell has something to display. The panel already has a "marks such a row" affordance for unusable ids, so the surface exists.
- **Verification**: no test asserts the error path; the happy path is covered.

11. **The live runtime discards every `play`/`pause` device error**

- **Severity**: MINOR (swallowed error on the transport path; the previous review's "stream errors after open are never surfaced", still open)
- **Location**: `crates/host/src/live.rs:359`, `383`, `386`, `411`, `430`, `460`
- **Trigger**: any ALSA error from `Stream::play`/`Stream::pause` (device unplugged, device busy, the stream was already in that state).
- **Wrong behaviour**: `let _ = a.handle.play();` — the transport flips to `playing`, `Snapshot.playing` says true, the playhead advances through the wall-clock pump, and **nothing is audible**, with no error anywhere. The device died and the shell reports a healthy playing transport. `OutputHandle` also has no error accessor, so a post-open stream error is structurally unreachable (`devices.rs:119-123` keeps the `err` `Arc` in a closure the handle never exposes).
- **Evidence**:
  ```rust
  // crates/host/src/live.rs:382-386
                      if is_play {
                          let _ = a.handle.play();
                      }
                      if is_stop {
                          let _ = a.handle.pause();
  ```
- **Fix**: route both results into `Snapshot::last_error` (which already exists and is used for render failures at line 449), and add an `OutputHandle::take_error()` mirroring `InputHandle`'s overrun counter so a post-open stream error is observable.
- **Verification**: `crates/host/src/live.rs:627-765` uses `HostHandle::spawn()` (the silent form), so no device call is exercised in tests at all.

12. **A seek during a take silently discards the take-finalize error**

- **Severity**: MINOR (swallowed error; the material still lands, but the diagnosis is lost)
- **Location**: `crates/host/src/lib.rs:2468-2470` (and the same shape at `2554-2556` in the test-only `replay_full`)
- **Trigger**: `record jam1`, then a `transport seek` (or an `undo`, or an `export`) while the take is running, on a pool where the finalize fails — a full disk at `WavWriter::finalize`, or a `PeakFile::write` that cannot create the `.peaks` temp.
- **Wrong behaviour**: the comment says the take is *"finalized … **and** reported"*, and the report is indeed kept (`self.last_take = Some(report)` inside `stop_recording`, line 966) — but the `Err` case, which carries the only description of what went wrong, is dropped on the floor. The user is told a take exists and is never told that its peaks are missing.
- **Evidence**:
  ```rust
  // crates/host/src/lib.rs:2468
          if self.recording.is_some() {
              let _ = self.stop_recording();
          }
  ```
- **Fix**: `if let Err(e) = self.stop_recording() { self.journal_error.get_or_insert_with(|| e.clone()); eprintln!(...) }` — the session already has a "last error the user must see" slot.
- **Verification**: the tests around it assert the *report*, never the error arm.

13. **A non-UTF-8 pool filename still yields `id: ""`, and the conform pass then writes dot-prefixed siblings beside it**

- **Severity**: MINOR (prior review finding, still open — the source is unaddressable and a hand-filled pool produces stray files)
- **Location**: `crates/media/src/pool.rs:217-221`, consumed by `crates/media/src/pool.rs:682-713`
- **Trigger**: a file named `tak\xFFe.wav` in the pool directory; `pool <dir>` runs `Pool::conform`, which filters on `sample_rate != session_rate || channels > 1` and calls `expand_one(&src.wav, rate, "")`.
- **Wrong behaviour**: the source is listed with `id: ""` — no `host v1` line can name it and no clip can reference it — and `expand_one` then writes its siblings as `path.with_file_name(".ch1.wav")` etc. (a hidden file, never indexed) while preserving the original as a numbered `.wav.pre<rate>` backup. The material is converted, duplicated and still unreachable.
- **Evidence**:
  ```rust
  // crates/media/src/pool.rs:217
              let id = wav
                  .file_name()
                  .and_then(|n| n.to_str())
                  .map(|n| n.strip_suffix(".wav").unwrap_or(n).to_string())
                  .unwrap_or_default();
  ```
  The prior review's fix was *"report in `PoolIndex::errors`"*; the `.unwrap_or_default()` is unchanged.
- **Fix**: `match wav.file_stem().and_then(|s| s.to_str()) { Some(s) if valid_id(s) => s.to_string(), _ => { index.errors.push((wav.clone(), "unusable source name".into())); continue; } }` — skip the source the way an unreadable one is skipped.
- **Verification**: `crates/media/tests/pool.rs` and `p1_2.rs` use ASCII stems only.

### NIT

14. **`MasterPlugin`'s disposer can install a dangling `out_node` id, after which `out_channels()` silently reports mono**

- **Location**: `crates/engine/src/plugins/master.rs:513` and `517-528`
- **Trigger**: `mount mixer`, `mount master`, `unmount mixer @0`, `unmount master @0` (both valid, FIFO order at equal frames via `Scheduler::schedule`'s `partition_point(f <= frame)`).
- **Behaviour**: the disposer captures `previous = mixer` at apply time, then at dispose does `remove_node(master)` (clearing `out_node`) followed by `set_out(previous)` — pointing at a node id that no longer exists. `out_channels()` then returns 1 (`index_of` → `None` → `unwrap_or(1)`) and `render_inner` fills silence. Benign today because `NodeId`s are never reused, but `set_out` should verify the node exists.
- **Fix**: `if dis.graph.out_node.is_none() && previous.is_some_and(|p| dis.graph.node_ids().contains(&p))`, or have `set_out` refuse an unknown id.

15. **`render_with_drain_aligned` reads the channel count before the flush that can change it**

- **Location**: `crates/engine/src/render.rs:994-995`
- **Trigger**: call `Engine::render_with_drain_aligned` with a mixer scheduled but not yet applied (e.g. from a shell that renders without going through `HostSession::render_with_drain`, which flushes at line 1874 precisely for this reason).
- **Behaviour**: `channels` is read at 995 *after* `flush_frames` but *before* `render_with_drain` → `render` → `channels_for_render` applies the mixer, so `head = latency * 1` while `out` holds `frames * 2` — the head trim cuts half as many samples as it should, shifting the bounce.
- **Fix**: flush first, then read `out_channels()`; the caller's flush is a workaround for a library-ordering bug.
- **Note**: the comment at `crates/host/src/lib.rs:1867-1873` documents the workaround, so this is a known-and-worked-around hazard, not a live defect in the shipped path.

16. **Dead branch in the export peak report** — `crates/host/src/lib.rs:2065`: `let db = if peak > 0.0 { 20.0 * peak.log10() } else { NEG_INFINITY };` sits inside `if peak > 1.0`, so `peak > 0.0` is always true. Harmless, but it reads as if a zero-peak case were handled here when it is not (it cannot be: `peak > 1.0`).

### Test gaps

- **Mid-block mount/unmount allocation** (finding 1) is invisible to both counting-allocator tests: `crates/engine/tests/spike_a.rs:851` primes with a render *before* arming the counter, and line 848 places the unmount at frame 100 000, outside the measured 8192 samples. This is the single most valuable missing test in the repo: arm the allocator, schedule one `Mount` and one `Unmount` inside the first block, assert 0.
- **Release-mode `u64` overflow in op validation** (finding 2). The existing test is `#[cfg(debug_assertions)]` and asserts the *panic*; nothing covers the release wrap.
- **No `Pool::recover` test with a 24-bit source** (finding 4) — `crates/media/tests/pool.rs` recovers only 16-bit and float takes.
- **No test arms `Pool::import`'s commit failure** (finding 5), so the ordering of `replace_sources` is unconstrained by tests.
- **No test that a `mount` with an out-of-range param is refused** (finding 6), which is why the ranges in `TONE_PARAMS`/`MASTER_PARAMS` look enforced when they are only enforced for `set_param`.
- **No subnormal/denormal assertion anywhere** (finding 8) — and none could exist as a content assertion, so it needs a timing or bit-pattern test.
- **`Pool::recover` is never called by the product.** `rg '\brecover\('` finds call sites only in `crates/media/tests/pool.rs`. The crash-recovery design the README describes ("crash-safe per-gesture journal" is separate, but the pool half — "a crashed take is finalized and its peaks rebuilt") has no entry point in `crates/host` or either shell: `HostSession::set_pool` calls `conform`, not `recover`. A crashed take therefore stays `finalized: false` in the pool listing forever. Either wire `recover` into `set_pool`/`load_session` or state in `docs/capabilities.md` that it is not yet called.
- **Device error paths are entirely untested** (finding 11): `HostHandle::spawn_with_audio` is never used in any test.

### Design risks

- **One reader thread and one 256 KiB ring per clip, rebuilt on every edit.** `wire_arranger` (`crates/host/src/lib.rs:1798-1844`) tears down and rebuilds *every* track's `ArrangerNode` on *any* `Arrange` op, and `ArrangerNode::new` spawns one `FilePlayer` (thread + `Spsc::new(1 << 16)` + up to 1.37 s of warmed audio) per clip (`arranger.rs:171-181`). A 30-minute arrangement with 200 clips therefore costs 200 threads and ~50 MiB per edit, and the retired readers linger up to 40 ms each (the EOF park at `stream.rs:183-185`) before they free themselves. The 0.1 s seek claim in the README is measured without this tax.
- **`ArrangerNode::warm` can stall the live pump for 10 s.** `warm()` (`arranger.rs:59-69`) sleeps in 2 ms steps up to a 10 s deadline, and it runs on the actor thread inside `live::fill_audio` → `session.render` → `wire_arranger`. A single arrangement edit during playback can therefore empty the output ring for up to 10 s. The same 10 s bound exists in `HostSession::warm_player` (`crates/host/src/lib.rs:1371-1380`).
- **`TempoMap` segments grow without bound and are read per block.** Every `set_tempo` appends a segment (`clock.rs:46-58`); `beat_at` is O(segments) and is called twice per block by `EuclideanGen::render` and by `ClockOutNode::first_tick_at_or_after`, which additionally calls `frame_at` (also O(segments)) in a `while` loop per block. `live::publish` clones the whole map on every tick (`live.rs:563-565`), 250×/s. A session with thousands of tempo changes pays a linearly growing cost on the render path and in the shell.
- **`Interner` leaks by design, and a rebuild leaks a fresh set.** `media::clip_editor.rs:37-43` `Box::leak`s every unique id; `HostSession::rebuild` creates a new `HostSession` (hence a new `ClipEditor` and interner) on every seek, undo, redo and export, and the old interner is never freed. Long sessions with many undos leak steadily.
- **`MediaSession` vectors grow without bound.** `splices`, `bounces` and `exports` (`media_ops.rs:83-89`) are appended per op and per replayed op, and a full replay during a seek re-appends every historical entry. They are observability records today, but nothing bounds them.
- **A valid-looking `add_clip` whose source region runs past the file bricks the session.** `validate_clip` never checks `src_start + src_len` against the file, so `arrange add_clip t0 c0 s1 9999999999 100 0 0 0 1.0` is accepted and logged; `ArrangerNode::new` then fails in `FilePlayer::start_looped_anchored` → `reader.seek_frames(start)?` (`stream.rs:121`), so `wire_arranger` → `render()` returns `Err` for every subsequent bounce, export, seek and undo, and `save` persists it. The failure is loud and the session is recoverable with a `delete`, but the error surfaces three commands later than the mistake and the log says the edit succeeded.
- **A valid-looking clip id is shared between two clips of one track.** `ArrangerNode::readers` is a `HashMap<Id, ClipReader>` (`arranger.rs:55`), so if two clips on one track ever shared an id both would pop from one reader and the alignment `debug_assert` at `arranger.rs:262-270` would fire. `Timeline` enforces global clip-id uniqueness (`clip_id_exists`, `timeline.rs:413-417`), so this is currently unreachable — but the arranger's own module doc claims "one reader per active clip *instance*", which the map's keying does not deliver.

### Checked and clean

Read end to end and found sound, with the noted caveats:

- **Stereo device output** (`devices.rs:259-293`) — the previous review's critical #1 is genuinely fixed and genuinely tested: `output_duplicates_mono_source_across_channels` asserts the exact 8-sample layout, plus passthrough, downmix, starvation, partial-frame and zero-channel cases. `fill_input` correctly pushes interleaved frames and `InputHandle::channels` carries the stream's own count.
- **`Spsc` ring** (`ring.rs`) — the count-based Release/Acquire protocol, per-field 64-byte padding (the previous review's false-sharing minor is fixed) and the compile-time `offset_of` assertions are all correct. The `try_push`/`try_pop` ownership argument holds.
- **The player-retire free** (`stream.rs:176-186`) — the previous critical #2 is genuinely fixed: the reader parks at EOF holding its `Arc` clones until `stop`, so the 256 KiB ring always frees on the reader's own thread. `Drop` detaches rather than joins.
- **`data_bytes`' >4 GiB guard** (`wav.rs:501-522`) — the previous critical #3 is genuinely fixed, with `checked_mul` and a `-37` margin that covers the RIFF pad, and a test at the boundary (`data_bytes_guard_refuses_oversized_take`).
- **`parse_script` operand handling** (`crates/host/src/lib.rs:3196-3704`) — the previous critical #4 is genuinely fixed: every arm goes through `word()`/`exact()`/`port_ref()`, `arrange` uses the `s(i)` helper, and `crates/host/tests/parse_script_robustness.rs` covers 18 truncated lines plus bad operands.
- **`render()` returning `Result`** (`crates/host/src/lib.rs:1854-1859`) and the mixer-unmount state clear (`:1395-1408`) — the previous major #6 is genuinely fixed; `wire_pending`/`wire_arranger` both propagate, and `arrange requires the mixer` is now an `Err` rather than a panic.
- **`ArrangerNode::new` rate-mismatch refusal and `validate_clip`** (`arranger.rs:128`, `141-146`) — the previous major #5's minimal fix is in place, and `ArrangerNode::new` now validates hand-built `Track`s (the previous minor).
- **`MaxWindow`** (`master.rs:148-207`) — the monotonic deque's ring arithmetic is right: the domination loop only shrinks, `at(len)` with `len ≤ cap` stays inside `cap+1`, and the front-drop restores the invariant.
- **`Resampler`** (`resample.rs`) — the history trim keeps `hist.len() == fed - base`, so `sample()`'s index is always in range; `ready()`'s `break` makes the `!pad` path total; `process`/`flush` order is enforced.
- **`Stretch`** (`stretch.rs`) — `hand_out`'s `acc[handed..produced]` is guarded by the `produced > handed` check and `target_len` is monotone in `fed`, so the slice can never invert; the tail read is clamped to `at_end`.
- **`TpdfDither`** (`dither.rs`) — the reserved-rail margin makes the post-dither clamp unreachable, and `q / 32767` round-trips through the 16-bit writer exactly.
- **`DriftCompensator`** (`drift.rs`) — `consumed` is clamped to `pending.len()` before the drain, which is what stops the tail-batch `floor(pos) > len` panic the comment describes.
- **`Engine::drain` / `render_with_drain_aligned` / `check_bounce_budget`** — the drain terminates on a bounded loop, `head` is clamped to `out.len()`, and `bounce 999999999999` is refused by name (the previous minor is fixed).
- **`fill_output`'s frame mapping, `intern`'s bounded leak, `switch` of the SPSC per-channel capture rings, `Snapshot`'s poisoned-lock fallback, `HostSession::save`'s parse-and-compare round-trip guard** (which correctly refuses a lossy session rather than writing one) — all read and sound.
- **The `tui-shell` and `iced-shell`** — 7,422 lines read for indexing, division, cast and panic sites. The production halves contain **zero** `unwrap`/`expect`/`panic!`/`assert!` outside tests (the only one is a `#[cfg(test)]` helper at `spikes/tui-shell/src/main.rs:3612`). The `View` zoom/scroll/clamp arithmetic is `saturating_*`-guarded, `playhead_frame >= view.start` is checked before the subtraction at `timeline.rs:614`, and `wave_script` refuses a temp dir or stem containing whitespace before interpolating either into a script line.

### Sweep accounting

| Sweep | Candidates examined | Confirmed |
|---|---|---|
| 1. Panics on reachable paths | 72 non-test sites (28 `debug_assert!`, 11 `assert!`, 33 `unwrap`/`expect`) | 3 real (findings 1, 2, 14); the `debug_assert!`s read as true invariants with documented release degradation, except the `SetClipFade` case which is a bare `+` |
| 2. Arithmetic / truncating casts | 61 `as <int>` sites + 14 hand-audited subtractions and divisions | 2 real (findings 6, 9); `add_signed`, `data_bytes`, `check_bounce_budget`, `quantize_frames` and `DriftCompensator::consumed` are all correctly guarded |
| 3. Float hazards | 34 sites (clamp/min-max pairs, `as f32`/`as u64` on floats, envelope accumulators, `log10`/`powf`/`sqrt` sites) | 1 real (finding 8); no `clamp(min > max)` anywhere, `partial_cmp().unwrap()` appears zero times, `NaN` is handled explicitly at `master.rs:210-212` and `host/lib.rs:2044-2051` |
| 4. Concurrency | 41 `Ordering::` / `unsafe` / `spawn` / `Drop` / `Mutex` sites | 2 real (findings 1, 7); every `Ordering` on the counters and flags is correct (`Release`/`Acquire` on `stop`/`eof`/`count`, `Relaxed` on monotonic diagnostics), zero `static mut`, zero `mem::forget`/`transmute`, one justified `unsafe impl Sync` |
| 5. Error swallowing | 88 sites (`let _ =`, `.ok()`, `unwrap_or_default`, `#[allow]`, `catch_unwind`) | 4 real (findings 10, 11, 12, and the swallowed `remove_file` inside finding 5); `#[allow(...)]` appears only on a `too_many_arguments` for a wide draw function; the single `catch_unwind` is test-only |
| 6. Resource and lifetime | 31 sites (BufWriter/WavWriter finalization, temp files, thread spawn/join, keyed collection growth) | 3 real (findings 3, 5, and the `.converting` temp leak inside 5); every `WavWriter` on a success path is `finalize()`d, `Drop` patches the header best-effort, and `PeakFile::write` is atomic via tmp+rename |

---

## 2. Adversarial verification (verbatim)

I'll verify each claim by reading the actual code.

Let me verify several details: the render-path invariant wording, the `Euclidean` param surface, and the host's `Pool::recover` callers.

Now let me verify the remaining details for each claim.

Now let me verify the remaining specifics for several claims.

Let me verify several remaining specifics.

Now let me check a few remaining details.

Let me verify a few remaining specifics.

Let me verify a few more specifics.

Let me verify the remaining specifics for claims 3 and 4.

Let me verify the last details.

Let me check a few final details for claims 1, 6, and 7.

Let me check the last two paths for claim 1.

Let me check one potential new finding around the bus owner being unmounted.

Let me verify a potential new finding about bus-owner unmount.

Let me verify a couple of details for a possible new finding.

Let me verify one potential new finding.

I have verified enough. Let me do one last check on the `arrange` path reaching `set_clip_fade` from a script and whether `bounce` also flushes.

I have enough evidence for all verdicts.

## Verdicts

### `PARTLY 7-mechanical#1`

The code reading is right; the trigger is not reachable through the claimed path.

**Right:** `crates/engine/src/render.rs:1148-1161` is the only gate, and `changes_master_width` (`render.rs:905-932`) returns `false` for anything whose declared audio-Out width equals `out_channels()`, and for every plugin with no audio Out (`declared == None` → `_ => false`). `euclidean` (`euclidean.rs:12-17`) declares only a `Trigger` Out, so a scheduled `euclidean` mount/unmount would indeed reach `apply_event` → `apply_mount` → `graph.add_node` (`graph.rs:870-871`, `RingDelay::with_capacity(4096*2)` = 8192 `f32` = 32 KiB) or `remove_node` on the render stack.

**Wrong — the trigger.** `crates/host/src/lib.rs:2716-2723`: `process` renders `(frame - now)` frames *before* `apply`, so the mount is scheduled at `at_frame == clock.frame()`, not strictly inside a future block. The next `render` call then reaches `Engine::render` (`render.rs:945-950`) → `channels_for_render()` → `flush_scheduled()` (`render.rs:1084-1095`), whose loop condition is `if frame > self.clock.frame() { break; }` — `500 > 500` is false, so the event is popped and applied on the **control** stack before `render_block` is ever entered. `bounce` additionally calls `self.engine.flush_scheduled()` explicitly at `lib.rs:1874`. So `mount euclidean … @500` + `bounce 2000` never reaches the render stack.

The render-stack path is only reachable when the event's frame is **strictly greater** than the clock at the flush point — i.e. via `Engine::schedule_unmount(name, future_frame)` or `replay_from` of a log carrying future frames, both engine-API-only (the host's `unmount`/`mount` always use `clock.frame()`). `crates/engine/tests/spike_a.rs:296-303` is exactly such a case (`schedule_unmount` at `96_100`, rendered in one `render(at+64)` call) — so the defect is real in the engine API but not via the `host v1` script the claim names, and the CRITICAL "live device underrun" framing does not follow: the live pump (`live.rs:486`) is a *host thread*, not the cpal callback (`devices.rs:261` only pops an Spsc ring), and no host command schedules a future-frame mount.

The test-gap half of the claim is accurate: `spike_a.rs:851` primes with `e.render(512)` and `:848` schedules at frame 100 000; `parked_arrangements.rs` covers only the `Arrangement` arm.

### `PARTLY 7-mechanical#2`

**Right:** `crates/media/src/timeline.rs:759` is `if fade_in + fade_out > self.tracks[ti].clips[ci].src_len` — a plain `u64 +`, and `parse_arrange`'s `u(i)` (`lib.rs:3680-3688`) has no bound. In debug this panics the overflow check *while `ClipEditor::apply` holds the timeline lock* (`clip_editor.rs:550-556`) — confirmed, and the existing test at `lib.rs:4102-4118` is precisely that, `catch_unwind`-wrapped. In release the sum wraps to 0, the check passes, and the op is written and logged via `arrange_logged` (`lib.rs:558`). All of that is right.

**Wrong — the release consequence.** The claim says `ArrangerNode::new` rejects the clip in `validate_clip` (`timeline.rs:349`), so every later render fails. But `validate_clip` at `timeline.rs:349` is the *same unchecked addition* — `c.fade_in + c.fade_out > c.src_len` — so in release it wraps identically and the check **also** passes. I traced `wire_arranger` (`lib.rs:1805-1811`) → `ArrangerNode::new` → `validate_clip` (`arranger.rs:128`): no error, `render()` returns `Ok`. The claim's own Fix section admits `validate_clip` has "the same unchecked addition", which contradicts its Wrong-behaviour section. The actual release consequence is a **silenced clip** (`fade_gain`, `arranger.rs:92-105`: `off / u64::MAX as f32` → ~0), not a dead session. That is a real defect, but much smaller than claimed.

The "the crate's own test comment asserts the opposite of what ships" framing is also a misread: the comment at `lib.rs:4055-4057` says release has "no reachable **poison** path", which is true — the release path corrupts the value, it does not poison. The test is not wrong about its own scope.

### `CONFIRMED 7-mechanical#3`

`crates/media/src/stream.rs:322` `pending: VecDeque::with_capacity(8)`, drained at `:355-361` with an unbounded `while let Some(cmd) = mbox.pop_front() { self.pending.push_back(cmd) }`, called once per block from `PlaybackNode::render` (`:382`) — i.e. inside `render_into`.

Trigger: `host v1` with `play <f> ch0`, then nine `splice <frame> <f> 512` lines, then `bounce 2000 <out>`. `lib.rs:1522-1530` pushes each `SpliceCmd` (owning a `FilePlayer`, constructed + warmed at `:1518-1519`) into the unbounded `Arc<Mutex<VecDeque<SpliceCmd>>>` mailbox; `HostCommand::Splice` has `at_frame() == None` (`lib.rs:412-427` omits it), so `process` renders nothing between them and all nine sit in the mailbox. The first `PlaybackNode::render` in the bounce's first block drains all nine into a capacity-8 `VecDeque` — the 9th `push_back` reallocates and moves eight `SpliceCmd`s on the render stack.

Not covered: `spike_b.rs:499-520` arms the counting allocator but primes with `rig.e.render_into(&mut out)` at `:513` first, and only ever schedules one splice (`:510`).

Severity note: the reviewer calls this MAJOR; the allocation is one `realloc` of a small `VecDeque` per burst, not a per-block cost. The invariant violation is real.

### `PARTLY 7-mechanical#4`

**Right:** `crates/media/src/wav.rs:409` `let data_bytes = data_bytes(frames, h.channels, h.bits == 32)?;` with `data_bytes(frames, channels, float)` at `:505-506` computing `bytes_per_sample = if float {4} else {2}`. For a 24-bit file `h.bits == 32` is false → 2 bytes/sample, while `frames` at `:404` correctly uses `h.block_align()` = `channels * 3`. So `f.set_len(h.data_offset + data_bytes)` at `:422` cuts a 24-bit file to 2/3, and `:414-421` patches both size fields to match — permanent and self-consistent. 24-bit is a first-class format (`wav.rs:119`, `read_into` at `:241`, `write_pcm24` test fixture at `:812`), and `Pool::recover` calls this for any `!src.finalized` source (`pool.rs:252-253`).

**Wrong — reachability and the "first-class in the pool" premise.** A 24-bit file can only be `!finalized` if its declared size mismatches its length, and the pool's own writers only ever emit 16-bit or 32-bit float (`WavWriter::create` / `create_float`; `pool.rs` never writes 24-bit). `Pool::import` of a 24-bit mono file at the session rate takes the byte-for-byte copy branch (`pool.rs:479-485`) — the copy is well-formed, so `is_finalized` is true and `recover` skips it. `Pool::conform` likewise leaves it alone. So the file is only reachable if a *hand-placed* 24-bit file is also *torn* — two independent preconditions, not "the ordinary way to get there."

The reviewer's own caveat is correct and decisive: `rg '\brecover\('` finds `Pool::recover` called only from `crates/media/tests/pool.rs:74,97` — no product path. This is a real bug in an unreferenced public API, not a MAJOR data-loss defect in the product. The severity is wrong; the code reading is right.

### `PARTLY 7-mechanical#5`

**Right about the ordering.** `crates/media/src/pool.rs:486-494`: `self.replace_sources(&id, 1);` runs at 486, `fs::rename(&tmp, &dest)` at 487. `replace_sources` (`:576-602`) ends in `let _ = fs::remove_file(&path); let _ = fs::remove_file(path.with_extension("peaks"));` (`:598-599`), swallowing errors. The multi-channel path repeats it at `:423` before the rename loop at `:426`. The doc at `:573-575` — "Called only once the new material is safely written, so a failed import changes nothing" — is contradicted by its own call site: the material is written to a `.converting` temp, not into place.

**Wrong about the trigger.** The claimed causes are not constructible. `fs::rename` within one directory on the same filesystem does not fail for ENOSPC in the ordinary case (the temp was just written; the rename is a metadata-only operation), and `fs::rename` on Windows *does* replace an existing file (`std::fs::rename` uses `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`) — the claim's "Windows' refusal to replace an existing file" is factually wrong. A read-only pool directory would fail at the earlier `fs::copy`/`write_channel` (`:472`/`:480`), before `replace_sources`. No test injects a commit failure, and I could not construct one from the code.

The residual is a narrow ordering smell (delete-before-commit is the wrong order; a crash between 486 and 487 also loses the old take), not a demonstrable MAJOR data-loss path. The suggested fix — move `replace_sources` after the commit — is still correct and worth doing.

### `PARTLY 7-mechanical#6`

**Right:** the only mount-param check is finiteness (`render.rs:231-235`), confirmed. `mixer_factory` (`mixer.rs:583-585`) is the one plugin with a range bound on a mount param, and `lib.rs:2706-2710` adds a host-side check for the mixer only.

**Trigger 1 (`steps=4000000000`) — right:** `euclidean_factory` casts `steps` to `u32` unchecked (`euclidean.rs:115`), `euclid` does `vec![false; n]` with `n = steps` (`euclidean.rs:23-25`), and `Euclidean::apply` then `pattern.clone()`s it (`euclidean.rs:82`). "steps" is in `HOST_PARAMS` (`lib.rs:72`) so `in_list` passes. A 4 GiB + 4 GiB allocation.

**Trigger 2 (`pulses_per_beat=4000000000`) — right:** `step_beats = 1.0 / pulses_per_beat.max(1)` (`graph.rs:481`), and the `for step in s0..s1` loop at `:486-498` runs `(b1-b0)/step_beats` iterations per block, each calling `TempoMap::frame_at` at `:494` for pattern hits. `pulses_per_beat` is in `HOST_PARAMS` (`lib.rs:75`). A real render-path stall.

**Trigger 3 (`blip_len=4000000000`) — wrong.** `tone_factory` does cast unchecked (`tone.rs:86`) and `TONE_PARAMS` declares `[1.0, 1_000_000.0]` (`tone.rs:33-36`), but `ToneGen::has_tail()` (`graph.rs:607-609`) is `self.blips.iter().any(|b| b.is_some())` — it reports a tail only when a blip is *currently sounding*. A `tone` mounted with a huge `blip_len` and no note source never gets a note (`render` at `:620` only schedules from `io.notes_in`), so no blip is ever created, `has_tail()` is permanently false, and the drain is a no-op. The claim's stated trigger (`mount tone blip_len=4000000000 @0` alone) does not produce the claimed consequence; it needs the full `euclidean → scale → tone` chain patched first. The defect (unchecked cast on a mount param) is real; this specific trigger is wrong.

### `PARTLY 7-mechanical#7`

**Right about the code:** `ClockOutNode::render` calls `log.take_due(...)` (`clock_out.rs:242`), which locks `TransportLog::queue` at `clock_out.rs:113-115`; and when the scratch is non-empty it locks `self.sink` at `clock_out.rs:283-287` and calls `device.send(&self.scratch, block.frame)` at `:289` **inside** that lock. Both `.expect()`s are panic paths. The host's counterparts are at `lib.rs:2505-2519`. Two `Mutex` acquisitions per block is exactly what the code does.

**Wrong on two of the three consequences.**

*Contention:* the claim says the transport log's writer is "the host's control path, not the same thread." In the shipped architecture it *is* the same thread: `TransportLog::push` is called from `HostCommand::TransportPlay`/`TransportStop` (`lib.rs:1426`, `:1432`), which run on the host/pump thread, and `take_due` runs from `fill_audio` (`live.rs:486`) on that same thread — the cpal callback (`devices.rs:261-277`) never touches either mutex. The module doc at `clock_out.rs:277-278` says "with a single sender it is uncontended", and for the shipped topology that is accurate. `detach_midi`/`restore_midi` (`lib.rs:2505-2519`) also run on that same thread (they wrap `export`/`replay_to_kind`, both `&mut self` methods), so they cannot block a render. Cross-thread contention would require a future shell writing transport from another thread — the claim itself concedes this ("once a shell is allowed to push transport itself").

*Poison:* a panic inside `MidiSink::send` would poison the slot, and the next `.expect` would panic — that part is correct as a trait-boundary statement. But the only shipped implementation, `MidiOut::send` (`midi.rs:277-289`), is `ring.try_push` plus two atomic increments; it cannot panic. The claim concedes this too ("today's blast radius is one uncontended CAS").

*Invariant:* the claim cites "invariant 1: lock-free". The actual stated invariants (`docs/architecture-explainer.md:100-108`) are "every mutation is logged" and "rendering is a pure function of the log". The allocation-free property (`README.md:80`, `architecture-explainer.md:319` "never allocates and never blocks") is a design rule, not one of the two numbered invariants. An uncontended `std::sync::Mutex` on the same thread is a CAS, not a block.

The finding reduces to: the render path takes two uncontended mutexes where a lock-free handoff would be idiomatic. That is a design critique, not a MAJOR defect, and the trigger is "mount clock_out and render" with no user-visible failure.

### New findings

**`crates/engine/src/render.rs:1088-1094` — `flush_scheduled` drops the frame-alignment guarantee for a `Mount` whose `at_frame` is in the past by more than one call.**

`flush_scheduled` applies every queued event with `frame <= clock.frame()` in one pass, with no width check. A `Mount` of a stereo-width plugin queued at frame 0 and flushed at clock frame 100 000 changes `out_channels()` from 1 to 2 *after* the caller's buffer was already sized — the exact hazard `changes_master_width` was written to prevent (`render.rs:892-904`), but only on the `render_block` path, not on the `flush_scheduled` path.

```rust
pub fn flush_scheduled(&mut self) {
    for event in std::mem::take(&mut self.parked) {
        self.apply_event(event);          // no changes_master_width gate
    }
    while let Some(frame) = self.scheduler.peek_frame() {
        if frame > self.clock.frame() { break; }
        let event = self.scheduler.pop().expect("peeked");
        self.apply_event(event);          // no changes_master_width gate
    }
}
```

Trigger: `e.schedule_unmount("mixer", 0)` (or any width-changing mount) followed by advancing the clock past it without rendering — e.g. `e.mount("mixer", &[("channels", 2.0)]); e.seek(100_000); e.render(512)`. `seek` (`render.rs:939-941`) places the clock without rendering, so the mount is still queued; `render` → `channels_for_render` → `flush_scheduled` applies it, and `graph.out_channels()` becomes 2 while `render_into` computed `block_samples` from the pre-flush value of 1 at `render.rs:1070-1071`. The block is then filled from a stereo bus into a buffer sized for mono — `render_inner`'s `out.copy_from_slice(&self.audio_out[i][..out.len()])` (`graph.rs:1282`) reads `out.len()` samples from the interleaved stereo buffer, so it does not panic, but the frame count is halved relative to a replay that never skipped the flush. The same asymmetry is why `rebuild` at `lib.rs:2603` places the clock with `seek` before `set_tempo` — the codebase already knows `seek` decouples the clock from the queue; it just has no guard here.

I did not run this; the trace is from reading `render.rs:1084-1095`, `1070-1071`, `graph.rs:1279-1284`, and `arranger`-independent width maths. It is worth a test that arms a counting allocator *and* a width assertion across `seek` + `flush_scheduled` — the two paths the existing no-alloc test never combines.
