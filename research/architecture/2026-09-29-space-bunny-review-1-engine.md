# Space-bunny external review — engine crate

> **Disposition.** External deep review, 2026-09-29, against `49e8774` (clean tree). Reviewer:
> Space Bunny (`opencode/space-bunny-free`) driven through OpenCode by DeepSeek Harness; the
> supervisor (DeepSeek-V4.1-Flash) wrote the scope, then handed every CRITICAL/MAJOR claim to an
> **independent adversarial verifier** that had to falsify it before it counted.
>
> This file is the **engine crate** scope, verbatim: the reviewer's report, then its verifier's
> verdicts. What was done about each finding — fixed, deferred or refuted — is in
> [the index](2026-09-29-space-bunny-review.md). Findings are the reviewer's, not the
> supervisor's: read the verdicts before acting on any single claim.

---

## 1. Reviewer's report (verbatim)

## engine review

### CRITICAL

1. **`replay_from` cannot replay a log that contains a remount of the same plugin — the engine writes logs it refuses to read back**

- **Severity:** CRITICAL (invariant 2: the session log is the document, and a document the engine itself produces cannot be replayed). Blast radius today is limited to engine consumers: the shipped host's load/seek path re-issues `HostCommand`s instead of calling `replay_from`.
- **Location:** `crates/engine/src/render.rs:224-228` (the guard), `crates/engine/src/render.rs:666-687` (the replay loop).
- **Trigger:** any log of the shape `Mount p` … `ScheduleUnmount p` … `Mount p`. The live engine produces exactly this: `unmount` schedules an `Unmount` and clears `disposers`/`node_of` at apply, so a later `mount` of the same name succeeds — the engine's own test `remount_reproduces_identimal_signal` (`crates/engine/tests/spike_a.rs:337-353`) builds precisely such a log.
- **Wrong behaviour:** `replay_from` returns `Err("plugin 'p' is already mounted (one instance per name in spike A.5)")` and schedules nothing past that event, so the session fails to load/replay at all — the log-order state of the *engine* is consulted instead of the lifecycle state of the *log*.
- **Evidence:**
  ```rust
  // render.rs:224
  if self.disposers.contains_key(name) || self.scheduled.contains(name) {
      return Err(format!(
          "plugin '{name}' is already mounted (one instance per name in spike A.5)"
      ));
  }
  ```
  ```rust
  // render.rs:674-687 — replay validates each Mount against the *present* state
  // and never clears `schedled` for an unmount it just replayed.
  Event::Mount { plugin, params, at_frame } => {
      self.validate_mount(plugin, params)?;
      self.scheduler.schedule(*at_frame, SchedEvent::Mount { plugin, params: params.clone() });
      self.scheduled.insert(plugin);
  }
  Event::ScheduleUnmount { plugin, at_frame } => {
      self.scheduler.schedule(*at_frame, SchedEvent::Unmount { plugin });
  }
  ```
  Walk it with `euclidean` as `p`: event 1 validates (clean) and inserts `euclidean` into `scheduled`; event 2 schedules an `Unmount` but leaves `scheduled` untouched (the `Unmount` is not *applied* during the loop — nothing renders yet); event 3 hits `self.scheduled.contains("euclidean") == true` → `Err`. The whole replay is abandoned, including every event before it.
- **Fix:** validate against a replay-local lifecycle set rather than the live scheduling state — `let mut live: HashSet<&str> = HashSet::new()`, insert on `Mount`, **remove on `ScheduleUnmount`**, and have `validate_mount` (or a `replay_from` wrapper) consult that instead of `self.scheduled` for the "one instance per name" rule. (`self.disposers` is empty on a fresh engine, so it is not the problem.) A regression test is one line away: take the log from `remount_reproduces_identical_signal` and `replay_from` it.
- **Verification:** read `validate_mount`, `apply_mount`, `apply_unmount`, `replay_from`, and every test that calls `replay_from` (`spike_a.rs:204`, `phase1_mixer.rs:172`, `drain.rs`, `arrangement_ops.rs:109/132`, `fundsp_synth.rs:217/263`) — none replays a log containing an unmount. Grepped the whole repo: no non-test caller of `replay_from` exists, which is why nothing has tripped over it.

### MAJOR

2. **A patch between two mounted plugins is logged even when the graph will refuse it as backward-ordered — the edit is in the log and never in the audio**

- **Severity:** MAJOR (the inverse of invariant 3: a *logged* mutation that never happens, silently).
- **Location:** `crates/engine/src/render.rs:322-384` (`validate_patch` — no order check), `crates/engine/src/render.rs:412-414` (`apply_patch` — `debug_assert` only), `crates/engine/src/graph.rs:896-900` (the rule it violates).
- **Trigger:** mount the mixer **before** a source and then patch the source into it — legal in the `host v1` language the TUI console accepts, e.g. `mount mixer channels=2 @0` / `mount tone @0` / `patch tone.audio mixer.ch0 @0`. Nodes are appended in mount-apply order, so the mixer is index 0 and the tone index 1. A remount produces the same shape: `unmount tone` + `mount tone` + `patch tone.audio mixer.ch0` puts the tone *after* the mixer.
- **Wrong behaviour:** `Event::Patch` is in the log; at apply, `graph.connect(1, "audio", 0, "ch0")` returns `Err("connect: patch cords must go forward")`, which is only `debug_assert`ed. Release: the mixer's channel 0 is never fed, the master is silent from that frame on, and nothing anywhere reports it. Debug/test builds: the assert panics inside `render_block` on the pump thread, which nothing catches.
- **Evidence:**
  ```rust
  // graph.rs:896-900
  let fi = self.index_of(from).ok_or("connect: unknown 'from' node")?;
  let ti = self.index_of(to).ok_or("connect: unknown 'to' node")?;
  if fi >= ti {
      return Err("connect: patch cords must go forward (topological order)".into());
  }
  ```
  ```rust
  // render.rs:412-414 — the only report is a debug assert
  if let Err(e) = self.graph.connect(from_node, from.1, to_node, to.1) {
      debug_assert!(false, "scheduled patch refused at apply: {e}");
  }
  ```
  `validate_patch` checks existence, direction, kind, and channel count — never order, even when both endpoints are already mounted and `graph.index_of` could answer it in O(n).
- **Fix:** in `validate_patch`, when both endpoints are mounted, compare graph indices and `Err` before logging (`from` must precede `to`); when one side is a queued mount, compare mount-apply order (a `HashMap<&'static str, u64>` sequence number assigned in `apply_mount`). Additionally, record apply-time refusals in a `Vec<String>` the host can drain, so a log-order error is observable in release instead of only asserted in debug.
- **Verification:** every engine test that wires `tone → mixer` mounts the mixer **last** (`spike_a.rs:78`, `phase1_mixer.rs:64`, `drain.rs:61`, `fundsp_synth.rs:80`) — the hazard is avoided by convention everywhere it is exercised. The only order test, `connect_rejects_type_mismatch` (`graph.rs:1375-1401`), calls `Graph::connect` directly, never through `Engine::patch`. The host hits the same constraint from the other side and works around it with `insert_before` (`crates/host/src/lib.rs:1483-1490`, whose comment says "A player appended after a materialized mixer would make its cord backward — every later render fails"), which confirms the rule is real and unguarded from the user's side.

3. **A finite mount param can put a permanently-NaN oscillator on the bus, and the meters report it as silence**

- **Severity:** MAJOR (silent wrong output on a documented, user-reachable path; the one sanitiser in the tree is in a plugin that is not in the default chain).
- **Location:** `crates/engine/src/plugins/scale.rs:85` (pitch assembled, unbounded), `crates/engine/src/graph.rs:623` (frequency), `crates/engine/src/plugins/mixer.rs:475` and `:491` (NaN-blind meter fold).
- **Trigger:** `mount scale count=1 degree0=2000 @0` (or `root=1500`). Every value is finite, so `validate_mount` accepts it; `Scale` declares no `ParamDef` surface at all (registered with `&[]`), so nothing bounds it.
- **Wrong behaviour:** `pitch = 2000` → `440.0 * 2f32.powf(2000.0/12.0)`. The overflow threshold is `pitch > ~1412` semitones (`2f32::powf` saturates at 128, and `440·2^(p/12) > 3.4e38` from p ≈ 1412). So `freq = +inf`; `inc = TAU·inf/sr = inf`; sample 0 is `sin(0) = 0`, sample 1 is `sin(inf) = NaN`, and `NaN + inf` stays NaN for the rest of the session. Every downstream buffer — mixer sums, master meters, the output copy — is NaN from the first trigger onward.
- **Evidence:**
  ```rust
  // graph.rs:620-624 (ToneGen::render)
  for note in io.notes_in {
      let freq = 440.0 * 2f32.powf(note.pitch / 12.0);
      self.schedule(note.offset, self.blip_len, freq);
  ```
  ```rust
  // mixer.rs:474-475 and 490-491 — f32::max *ignores* NaN and returns the other operand
  let g = v * self.gains[ch];
  *peak = peak.max(g.abs());
  ...
  let full = l.abs().max(r.abs());
  self.peaks[self.channels] = self.peaks[self.channels].max(full);
  ```
  So the audio is NaN while `channel_peak`/`master_peak` read `0.0` (the shells show −∞ dB on a channel that is pure NaN). `MasterNode` is the only node with a `finite_or_zero` input policy (`master.rs:210-212`, `master.rs:392-393`) and it is not in the default profile; the graph's own output copy (`graph.rs:1282`, `out.copy_from_slice(&self.audio_out[i][..out.len()])`) does not sanitise. Consequence downstream: `export` refuses with "a NaN or infinity reached the mix" (`crates/host/src/lib.rs:2053-2058`) — the operator's only clue — and `bounce` has no such check at all (`crates/host/src/lib.rs:1684-1698` writes `&out` straight through), so a bounce silently produces a file of NaNs (a float WAV of crackle; as 16-bit, `NaN as i16 == 0`, i.e. a silent file).
- **Fix:** (a) bound the pitch where it is produced — `ScaleGen::render` should clamp `pitch` to a documented musical range (or the factory should reject an out-of-range `root`/`degree*`), so an absurd value is a refused mount rather than an infinite oscillator; (b) move the `finite_or_zero` policy the master already applies from inside one plugin to the graph's output copy, so *every* bus is protected by construction; (c) make the meter folds NaN-propagating (`if !g.is_finite() { *peak = f32::NAN; }` or a `non_finite` counter alongside the peak) — the host's export already documents the exact hazard this fixes.
- **Verification:** read `ToneGen::render`, `ScaleGen::render`, `MixerNode::render`, `MasterNode::render`, the graph's output copy, and the host's export/bounce measurement. `master.rs:713-731` (`non_finite_input_is_silenced_not_propagated`) proves the *master* heals NaN — and proves the sanitiser is not on the path the default chain takes. No test renders a pitch above the f32 overflow threshold or asserts a meter's behaviour on a NaN sample.

4. **`ToneGen`'s pending-onset queue has no wraparound: a note whose `offset` is outside the block indexes past the ring**

- **Severity:** MAJOR by class (an out-of-bounds index on the render path — invariant 1) with an explicit reachability caveat: **no in-tree producer can trigger it today.** It is reachable by any plugin that emits a note with `offset >= frames`, which nothing forbids and which `ToneGen::has_tail` already documents as a real case.
- **Location:** `crates/engine/src/graph.rs:588-595` (the write), `:630-644` (the drain), `:662-664` (the head reset).
- **Trigger:** a note event whose `offset` is `>= io.frames` (a legal `EventBuf` payload — `EventBuf` validates capacity, never offsets). Sequence: block A emits one note at `offset 0` (drains, `pending_head` 0→1) plus seven notes at `offset >= frames` (never drained, `pending_count = 7`); block A ends with `pending_count != 0`, so the head is not reset. Block B schedules any new note: `at = 1 + 7 = 8` on an 8-slot array.
- **Wrong behaviour:** `self.pending[8]` → index-out-of-bounds **panic on the render/pump thread**, taking down the render loop mid-block.
- **Evidence:**
  ```rust
  // graph.rs:588-594 — writes at head+count with no modulo; the guard only checks count
  fn schedule(&mut self, offset: u32, len: u32, freq: f32) {
      if self.pending_count < MAX_PENDING {
          let at = self.pending_head + self.pending_count;
          self.pending[at] = (offset, len.max(1), freq);
          self.pending_count += 1;
      }
  ```
  ```rust
  // graph.rs:662-664 — the head only returns to 0 when the queue fully empties
  if self.pending_count == 0 {
      self.pending_head = 0;
  }
  ```
  `head + count` is invariant under drains and grows by one per schedule, so the array is addressed as if it were linear, not circular. It happens to be safe today *only* because every offset the engine produces is `< frames` (`EuclideanGen` pushes `frame - block.frame` for `frame < block.frame + len`; `ScaleGen` passes it through) — an invariant enforced in one producer, relied upon in a consumer, and stated nowhere. The author's own `has_tail` comment (`graph.rs:603-606`) says "a malformed `offset >= frames` could never be drained by the per-sample loop", i.e. the malformed case was anticipated and left unhandled.
- **Fix:** make the ring honest — `let at = (self.pending_head + self.pending_count) % MAX_PENDING;` with the `count == MAX_PENDING` case dropping the *oldest* entry (`pending_head = (pending_head + 1) % MAX_PENDING; pending_count -= 1;`) before writing, so `head + count` can never exceed the array. Belt and braces: clamp `note.offset` to `io.frames` in `ToneGen::render` (and validate offsets in `EventBuf::push`/`insert_sorted`, which is where the platform-wide contract belongs).
- **Verification:** read the whole `ToneGen` lifecycle including the `pending_head`/`pending_count` invariant across blocks, plus both note producers (`EuclideanGen::render`, `ScaleGen::render`). Tests (`spike_a.rs:534-628` fan-in merge, `drain.rs`) all keep offsets inside the block, so no test can reach it. Not promoted to CRITICAL because I could not produce a trigger from the shipped plugins — it is listed in **Test gaps** as the contract hole it is.

### MINOR

5. **Mount params skip the declared parameter ranges that `set_param` enforces** — *incomplete fix of the previous review's Major #9 (the finiteness half landed, the range half did not)*

- **Severity:** MINOR (the loudest consequence — an over-full-scale mix — is caught at export, not silent).
- **Location:** `crates/engine/src/render.rs:229-235` vs. `crates/engine/src/render.rs:542-556`; `crates/engine/src/plugins/tone.rs:26-37` and `:84-87`.
- **Trigger:** `set_param tone gain 2.0` is refused (`"out of range [0, 1]"`, `TONE_PARAMS`); `mount tone gain=2.0` is accepted, logged, and applied. Same knob, two different answers.
- **Wrong behaviour:** the mount path applies unbounded values for any knob that also exists in the `ParamDef` catalog, because the catalog is consulted only by `set_param`. With the default chain a `gain=2.0` blip peaks at `2·√2/2 = 1.41`, so `export` refuses ("would clip … lower the mix or mount a master chain") with a message that points at the mixer, not at the mount that caused it.
- **Evidence:**
  ```rust
  // render.rs:229-235 — finiteness only
  // Mount params get a finiteness check (GLM-5.3 #9): `set_param` has one,
  // but a `mount ... NaN` would otherwise reach the plugin's apply silently.
  for (pname, v) in params {
      if !v.is_finite() {
          return Err(format!("mount param '{pname}' must be finite, got {v}"));
      }
  }
  ```
  ```rust
  // render.rs:549-556 — the range check `mount` never reaches
  if !value.is_finite() { return Err(format!("parameter '{param}' must be finite, got {value}")); }
  if value < min || value > max {
      return Err(format!("parameter '{param}' out of range [{min}, {max}]: {value}"));
  }
  ```
  `tone_factory` (`tone.rs:84-87`) does not clamp, and `fundsp_synth_factory` (`fundsp_synth.rs:196-201`) does not clamp `gain` against `FUNDSP_PARAMS` either (it does clamp `cutoff`/`q` inside the node, so the guarding is ad hoc and per-factory).
- **Fix:** in `validate_mount`, after the finiteness pass, check every param whose name appears in `self.params_table[name]` against that `ParamDef`'s `[min, max]` (names not in the table — `channels`, `steps`, `root` — stay the factory's business, which is exactly how `mixer_factory`'s `1..=MIXER_CHANNELS_SANITY` guard already works). Longer term, give `Plugin` a `mount_params()` surface so the range is declared once.
- **Verification:** read `validate_mount`, `set_param`, `params_of`/`ports_of`, every factory, and `TONE_PARAMS`/`FUNDSP_PARAMS`/`MIXER_PARAMS`. Tests: `phase1_mixer.rs:198-221` covers `set_param` refusal only; `mixer.rs:612-625` covers the *mixer's* mount-param bound only. No test asserts that a mount param is range-checked.

6. **A repeated identical `patch` is accepted and logged, and doubles the signal**

- **Severity:** MINOR (replay is faithful, so no determinism break — the level is simply wrong, and wrong by an amount that grows with each repeat).
- **Location:** `crates/engine/src/graph.rs:947-951` (unconditional push), `crates/engine/src/graph.rs:1201-1207` (fan-in sums).
- **Trigger:** the same `patch tone.audio mixer.ch0` twice in a script (trivial to do from the TUI console), or any host that re-issues a patch it already applied.
- **Wrong behaviour:** two identical audio cords into one input port are summed, so the source is +6 dB; for trigger/note inputs, `insert_sorted` inserts every event twice, so each blip is doubled (and the two copies can exhaust `ToneGen`'s 8-slot pending pool — see finding 4). The control single-driver rule at `graph.rs:920-927` shows the duplicate case was considered for control ports only.
- **Evidence:**
  ```rust
  // graph.rs:947-951 — no "is this cord already there?" question
  self.cords.push(PatchCord { from: (fi, from_port_idx), to: (ti, to_scratch), kind: in_port.kind });
  Ok(())
  ```
  ```rust
  // graph.rs:1201-1206 — every matching cord adds into the same buffer
  let src = &self.audio_out[cord.from.0][..ch * frames];
  let dst = &mut self.audio_ins[i][cord.to.1][..ch * frames];
  for (acc, s) in dst.iter_mut().zip(src) { *acc += *s; }
  ```
- **Fix:** keep a `HashSet<(NodeId, &str, NodeId, &str)>` of applied cords in the `Graph` and have `connect` return `Err("already patched")`; mirror it in the engine (a set of logged cord keys consulted by `validate_patch`) so the duplicate is refused *before* the log, per invariant 3.
- **Verification:** read `connect` in full and the fan-in loop; checked `spike_a.rs:534-628` (two *different* producers into one port — the case that is supposed to sum) and every other patch test. No test patches the same pair twice.

7. **The compressor's envelope decays into denormals, and every sample pays a `log10` + `powf` even when the compressor is transparent**

- **Severity:** MINOR (render-path cost and a classic realtime hazard; no test can see either).
- **Location:** `crates/engine/src/plugins/master.rs:335-345` (`compressor_gain_db`), `:397-404` (per-sample call site), `:218-224` (`lin_to_db`).
- **Trigger:** any session with `master` mounted. `env` is updated once per sample as `coeff*env + (1-coeff)*level`; with the default 120 ms release at 48 kHz (`coeff = 0.999826`) it takes ≈10.4 s of digital silence to fall from 1.0 to 1e-38, after which every multiply and add in the envelope update operates on a **denormal** (no FTZ/DAZ anywhere in the crate), which costs tens to hundreds of cycles per operation on the hardware that does not flush.
- **Wrong behaviour:** a silent passage after any audio runs the master in denormal arithmetic; combined with the unconditional `lin_to_db(env)` (`log10`) and `db_to_lin(comp_db)` (`powf`) per sample — 96 000 transcendental pairs per second at 48 kHz, paid even when `ratio == 1` and the material is 40 dB under the threshold.
- **Evidence:**
  ```rust
  // master.rs:396-404
  let level = l.abs().max(r.abs());
  let coeff = if level > self.env { self.attack_coeff } else { self.release_coeff };
  self.env = coeff * self.env + (1.0 - coeff) * level;
  let comp_db = self.compressor_gain_db(self.env);
  let comp = db_to_lin(comp_db) * makeup;
  ```
  ```rust
  // master.rs:218-224 — no flush-to-zero floor; -120 dB is returned for denormals too
  fn lin_to_db(lin: f32) -> f32 { if lin <= 0.0 { -120.0 } else { 20.0 * lin.log10() } }
  ```
- **Fix:** `if self.env < 1e-20 { self.env = 0.0; }` after the envelope update (or set FTZ/DAZ once via `_mm_setcsr`/`flush_to_zero` behind a cfg), and short-circuit the whole gain computation when `self.env <= 10f32.powf(self.threshold_db/20.0) || self.ratio == 1.0` so the common transparent case costs one compare instead of two transcendentals.
- **Verification:** read the whole compressor/limiter loop, `update_ballistics`, and the master tests (`master.rs:602-731`, all of which render short, loud, non-silent material — the denormal regime is never entered). `f32::max` in the meter folds (finding 3) is a separate NaN-blindness, not a denormal issue.

8. **`scale`'s `count` mount param is unbounded: one script line asks for a multi-gigabyte allocation**

- **Severity:** MINOR (control side, but a single legal line can abort or hang the process).
- **Location:** `crates/engine/src/plugins/scale.rs:69-71`.
- **Trigger:** `mount scale count=1000000000 @0` — finite, so `validate_mount` accepts it.
- **Wrong behaviour:** `Vec::with_capacity(1_000_000_000)` of `i32` = 3.7 GB, then a billion-iteration fill loop (seconds), then `self.degrees.clone()` into the node at apply (`scale.rs:45-49`) for another 3.7 GB. With overcommit the process survives and stalls; without it, the allocation error aborts the process.
- **Evidence:**
  ```rust
  // scale.rs:69-71
  let count = get("count", 5.0) as usize;
  let mut degrees = Vec::with_capacity(count);
  for i in 0..count {
  ```
  The identical hazard was recognised and fixed for the mixer one file over — `MIXER_CHANNELS_SANITY` exists, in its own words, "so `channels=1000000` cannot allocate gigabytes" (`mixer.rs:23-27`, enforced at `mixer.rs:583-588`) — and the host added `MAX_BOUNCE_BYTES` for the same reason. `scale` has no equivalent bound.
- **Fix:** clamp `count` (e.g. `1..=64`, with the same "sanity bound, not a design ceiling" framing) or return `Err` from `scale_factory`, matching the mixer.
- **Verification:** read `scale_factory` and the mixer factory side by side; the mixer test `factory_accepts_any_sane_width_and_refuses_the_absurd` (`mixer.rs:612-625`) is the pattern to copy. No test drives `count` at all — `scale` is registered with `&[]` params everywhere (`spike_a.rs:32-38`, `phase1_mixer.rs:25-31`, `drain.rs:21-27`).

9. **`Engine::new` accepts any tempo, so a 0 / negative / NaN bpm yields a silently dead tempo map**

- **Severity:** MINOR (the product hard-codes 120.0; `set_tempo` — the user-facing path — does validate).
- **Location:** `crates/engine/src/render.rs:154-158` and `crates/engine/src/clock.rs:25-34`; symptom at `crates/engine/src/graph.rs:482-497`.
- **Trigger:** `Engine::new(48_000, 0.0, 4)` (or `f64::NAN`, or a negative bpm). `Clock::new` → `TempoMap::new` stores it with no check, while `set_tempo` refuses exactly these (`render.rs:499-501`).
- **Wrong behaviour:** the tempo map's own arithmetic goes degenerate — `beat_at` returns 0 (bpm 0) or NaN, `EuclideanGen` computes `s0 = (NaN/step).floor() as i64 = 0` and `s1 = 0`, so the generator emits **no triggers at all** and the session is silent with no error; the shells' ruler (`spike/tui-shell` reads `tempo_at`/`beat_at` off the snapshot) shows NaN. `TempoMap::push` is likewise unvalidated and public.
- **Evidence:**
  ```rust
  // render.rs:154-158 — the initial tempo skips set_tempo's own policy
  pub fn new(sample_rate: u32, bpm: f64, beats_per_bar: u32) -> Self {
      let clock = Clock::new(sample_rate, bpm, beats_per_bar);
  ```
  ```rust
  // render.rs:499-501 — the policy that is not applied at construction
  if !bpm.is_finite() || bpm <= 0.0 {
      return Err(format!("tempo must be finite and positive, got {bpm}"));
  }
  ```
  (I checked the overflow candidate in this area and it is *not* a bug: in `frame_at`, `remaining < seg_beats` bounds the computed `frames` by the segment's own span, so `seg.start_frame + frames` cannot overflow for any positive bpm.)
- **Fix:** route the constructor through the same predicate (`assert!`/`debug_assert!` + a sane clamp, or make `TempoMap::push` reject non-finite/non-positive bpm), so the "tempo must be finite and positive" contract holds wherever a tempo enters the engine.
- **Verification:** read `TempoMap::{new,push,beat_at,frame_at}`, `Clock::new`, `Engine::new`, `set_tempo`, and the clock tests (`clock.rs:230-269`, all at 120 bpm). No test constructs an engine with a degenerate tempo.

10. **`replay_from` re-validates `Mount`, `Patch` and `Arrangement` but not `SetTempo` / `SetParam` / `ScheduleUnmount`**

- **Severity:** MINOR (needs a log the engine did not produce; `Event` is a public enum with public fields, so hand-built logs are legal).
- **Location:** `crates/engine/src/render.rs:706-734` (no validation) vs. `:498-501` (the policy skipped).
- **Trigger:** `replay_from(&log)` where the log contains `Event::SetTempo { bpm: f64::NAN, .. }` or `bpm: 0.0` — values `Engine::set_tempo` can never log.
- **Wrong behaviour:** the event is scheduled and applied, `tempo_map.push(at_frame, NaN, _)` lands, and every downstream beat computation goes NaN — a silent session with no error, which is exactly the outcome the `Arrangement` arm's comment ("fail loud … would otherwise … yield a session with no media state and no error") was written to prevent.
- **Evidence:**
  ```rust
  // render.rs:706-718 — scheduled verbatim, no finite/positive check
  Event::SetTempo { bpm, beats_per_bar, at_frame } => {
      self.scheduler.schedule(*at_frame, SchedEvent::SetTempo {
          bpm: *bpm, beats_per_bar: *beats_per_bar, at_frame: *at_frame,
      });
  }
  ```
- **Fix:** factor the `set_tempo` predicate into a free function and apply it in the `SetTempo` arm (and the `set_param` predicate in the `SetParam` arm), so replay's validation is the same validation the live path performs.
- **Verification:** read the whole `replay_from` match and `apply_event`; the host's own `host v1` parser does validate bpm (`crates/host/src/lib.rs:1546`), and the host's load path does not use `replay_from`, which is why this is latent.

11. **A failed `apply_mount` leaves the plugin name permanently "mounted", and `apply_unmount` will not clean up a node it has no disposer for**

- **Severity:** MINOR (latent — no in-tree plugin's `apply` can fail; reported because the error path is reachable the moment a plugin registers a handler, as `media::clip_editor::register_handlers` demonstrates is possible).
- **Location:** `crates/engine/src/render.rs:268-290` (early `?` on `apply`), `crates/engine/src/render.rs:475-493` (`node_of` removed only inside `if let Some(disposer)`).
- **Trigger:** a plugin whose `apply` returns `Err` after adding a graph node — e.g. one that calls `api.ctx`-visible `register_op_handler` (which `Err`s when the op is already claimed, `render.rs:583-585`).
- **Wrong behaviour:** `scheduled` keeps the name, so every later `mount` of it is refused with "already mounted" for the life of the engine; and because no disposer was stored, the already-added node stays in the graph with nothing able to remove it (`unmount` reaches `apply_unmount`, finds no disposer, and leaves `node_of`/the node in place) — an orphan that renders (or not) forever.
- **Evidence:**
  ```rust
  // render.rs:268-290 — the `?` returns before the bookkeeping that follows
  self.mounted_ports.insert(name, plugin.mounted_ports());
  self.mounted_params.insert(name, plugin.mounted_params());
  let (node, disposer) = { … plugin.apply(&mut api)? };
  self.scheduled.remove(name);
  self.scheduled_params.remove(name);
  ```
  ```rust
  // render.rs:475-493 — node_of is only cleared when a disposer exists
  fn apply_unmount(&mut self, name: &'static str) {
      self.scheduled.remove(name);
      self.mounted_ports.remove(name);
      self.mounted_params.remove(name);
      if let Some(disposer) = self.disposers.remove(name) {
          self.node_of.remove(name);
  ```
  (Note `scheduled_params` is cleared in `apply_mount` but not in `apply_unmount` — harmless today only because `patch`/`set_param` both require scheduled-or-mounted first, but it is the same omission.)
- **Fix:** make `apply_mount` transactional — collect `(node, disposer)` and only then commit `mounted_ports` / `mounted_params` / `scheduled` / `scheduled_params` / `node_of` / `disposers`; and have `apply_unmount` clear `node_of` and `scheduled_params` unconditionally.
- **Verification:** read `apply_mount`, `apply_unmount`, `validate_mount`, and all six in-tree `apply` implementations (`euclidean.rs:74`, `scale.rs:43`, `tone.rs:62`, `mixer.rs:541`, `master.rs:504`, `fundsp_synth.rs:168`) — none can return `Err` after `add_node`, which is exactly why no test covers this. Also read `media/src/clip_editor.rs:470-485`, the in-tree precedent for a failing `register_op_handler`.

12. **`render_into` and `Graph::render` have undocumented size preconditions; violating them is a debug panic or a silently stale tail**

- **Severity:** MINOR (no in-tree caller violates either; both are `pub`).
- **Location:** `crates/engine/src/render.rs:1069-1075` and `:1160-1167`; `crates/engine/src/graph.rs:1160-1170` and `:1229-1247`.
- **Trigger:** `engine.render_into(&mut buf)` where `buf.len() % out_channels != 0` (e.g. 4095 samples with a stereo master), or `graph.render(&mut out, block)` with `out.len() > out_channels * BLOCK`.
- **Wrong behaviour:** (a) the first case trips `debug_assert_eq!(written, out.len())` in a debug build and, in release, advances the clock by fewer frames than the buffer implies while leaving the tail samples **unwritten** (whatever was in the caller's buffer is passed off as audio). (b) The second case indexes `self.audio_ins[i][k][..ch * frames]` with `frames > BLOCK` on a buffer sized `ch * BLOCK` — an index-out-of-bounds panic in *both* profiles, from a `pub fn` whose doc says only "Render one block into `out`".
- **Evidence:**
  ```rust
  // render.rs:1102-1104, 1167 — integer truncation, then an assert that only debug sees
  let ch = self.graph.out_channels().max(1);
  let frames = out.len() / ch;
  ...
  debug_assert_eq!(written, out.len());
  ```
  ```rust
  // graph.rs:1235-1239 — the view is sliced to ch*frames against a ch*BLOCK buffer
  let io = NodeIO { audio_in: ins.get(0), audio_ins: ins, audio_in_count: ins.count(), ... };
  // AudioInputs::get: &buf[..ch * self.frames]
  ```
  Also: `Graph::render` with `out.len() < channels` computes `frames == 0`, so nodes render nothing and the final `out.copy_from_slice(&self.audio_out[i][..out.len()])` emits the **previous** block's samples.
- **Fix:** document the contract on both functions (`render_into`: "length must be a multiple of the master channel count; the remainder is an error"), return `Result` (or zero the tail and `debug_assert`) instead of truncating silently, and have `Graph::render` either chunk internally or `assert!(out.len() <= channels * BLOCK)` with a message naming the limit.
- **Verification:** traced every caller of both functions in the workspace (`Engine::render` sizes `frames * ch`; `Engine::drain` clamps to `BLOCK`; `Engine::render_chunk` splits by event frames, all ≤ `BLOCK`; tests chunk by `BLOCK * ch`). No in-tree caller violates the preconditions — which is why the existing no-alloc and drain tests pass.

### NIT

13. **`MixerNode::set_param` clamps `pan` but not `gain`/`master.gain`** — `crates/engine/src/plugins/mixer.rs:421-430`. `MasterNode::set_param` documents and implements "direct calls are clamped here so a node can never be steered out of its declared range" for all six params (`master.rs:312-315`); the mixer clamps one of five. Reachable only through the public `Graph::set_param` (the logged path range-checks first), so the asymmetry is a consistency gap rather than a defect. Fix: `value.clamp(0.0, 2.0)` on the two gain arms, matching the master's stated policy.

14. **The render-path `cum` uses `+` where its control-side twin uses `saturating_add`** — `crates/engine/src/graph.rs:1185` (`(base + self.nodes[i].latency()).min(MAX_PDC as u32)`) vs. `crates/engine/src/graph.rs:1100-1102` (`base.saturating_add(...).min(MAX_PDC as u32)`). A plugin declaring a latency near `u32::MAX` panics on arithmetic overflow in debug/test builds on the render path, and wraps in release, where the applied PDC then disagrees with `flush_frames()`'s (saturating) transit — i.e. the aligned-bounce head trim would be wrong. No in-tree plugin comes close (max 2048), but the clamp is already there and the addition should be inside it. Fix: `base.saturating_add(...)`.

15. **`let _ = audio_before;` in the render loop** — `crates/engine/src/graph.rs:1246` and `:1275`. A borrow-splitter whose first half is never used, kept alive by a `let _ =`. `split_at_mut(i)` already yields exactly the borrow the node needs (`audio_after[0]`); the placeholder reads like an unfinished edit.

16. **`OpMsg` is exported, documented and entirely unused** — `crates/engine/src/value.rs:33-56`, re-exported at `crates/engine/src/lib.rs:61`. Zero call sites and zero tests; the coercion rules it encodes (`U32 → u64`, `I64 → u64` via `try_from`, wrong-type → `None`) are re-implemented four times by hand (`crates/host/src/media_ops.rs:167-193`, `crates/media/src/clip_editor.rs:299-324`). Either use it where the fields are decoded (which would delete three of the four copies and get the `u64`/`f32` accessors under test) or drop it from the public surface.

17. **Scale degrees past index 9 all default to 22** — `crates/engine/src/plugins/scale.rs:83`: `_ => get("degree9", 22.0)`. `mount scale count=20` yields fifteen identical 22-semitone degrees. The fallthrough is a copy of the `degree9` arm rather than a deliberate clamp; a `count` above 10 should either be refused or documented as repeating the top degree.

### Test gaps

Each of these is a case the 326-test suite structurally cannot see, in the order I would add them:

- **Replay a log containing a remount** (finding 1). `crates/engine/tests/spike_a.rs:337-353` already *builds* the log; one `replay_from` on it would have failed on day one.
- **Mount the mixer before a source, then patch** (finding 2). Every engine test mounts the mixer last by convention; nothing exercises `Engine::patch` between two mounted plugins in the wrong order.
- **A repeated identical `patch`** (finding 6). `fan_in_merges_events_sorted` covers two *different* producers into one port, which is the case that should sum.
- **A note with `offset >= frames`** (finding 4). Needs a stub producer that pushes an out-of-block offset — exactly the "malformed offset" `ToneGen::has_tail`'s comment anticipates.
- **Mount params outside the declared `ParamDef` range** (finding 5). `TONE_PARAMS.gain` is `[0, 1]`; nothing asserts `mount tone gain=2.0` is refused the way `set_param` refuses it.
- **A pitch above the f32 overflow threshold** (finding 3), and separately a **meter assertion on a NaN sample** — the NaN-blind `f32::max` fold is invisible until something feeds it a NaN.
- **`render_into` with a length that is not a multiple of the master channel count** (finding 12) — a public API whose release-mode failure is a stale, unrendered tail.
- **`Engine::new` with bpm 0 / negative / NaN** (finding 9) and a **`TempoMap::push` with a non-positive bpm**.
- **A drain that crosses a scheduled unmount or mount.** `Engine::drain` bypasses `render_block` entirely, so no scheduled event applies during the tail window and a blip rings past its logged unmount frame. Bounded and deterministic, but undocumented and unasserted.
- **`dealloc` counting in the no-alloc harness.** All three counting allocators (`spike_a.rs:821`, `phase1_mixer.rs:366`, `fundsp_synth.rs:317`) count `alloc` and `realloc` and pass `dealloc` straight through, so the test named "the render path must not allocate" cannot observe the 32 KiB `RingDelay` plus plugin-state frees that `apply_unmount → Graph::remove_node` performs inside `render_block`.
- **`scale count=1e9`** (finding 8) — the mixer has `factory_accepts_any_sane_width_and_refuses_the_absurd`; the scale has no equivalent.
- **A mid-voice `set_param fundsp_synth env_len` change.** `crates/engine/tests/fundsp_synth.rs:222-224` claims the test covers "determinism, incl. an in-flight envelope rescale" — but the test changes `gain`, not `env_len`, so the rescale it names is untested (and is broken; see Design risks).

### Design risks

- **`FundspSynth` rescales a live envelope without clamping** — `crates/engine/src/plugins/fundsp_synth.rs:126` (`let env = self.remaining as f32 / self.env_len as f32;`) with `:137` (`"env_len" => self.env_len = …` leaving `remaining` alone). `set_param fundsp_synth env_len 100` (inside the declared `[1, 1e6]`) while a 20 000-sample voice sounds yields `env = 200.0` — a 200× burst, i.e. a very loud click and a mix that the exporter then refuses. `ToneGen` does this correctly by storing `len` per blip (`graph.rs:635-640`), so the fix is to copy that: clamp `self.remaining = self.remaining.min(self.env_len)` in the `env_len` arm. (Feature-gated and off by default, which is why it is a risk rather than a finding.)
- **The render path is a pump thread, not the device callback.** `crates/host/src/live.rs:428-454` renders on the pump thread and the device callback only drains an SPSC ring, so engine-side allocation/free costs jitter, not a dropout. This is worth stating in the invariant itself: "realtime-safe" is currently enforced against a thread that can afford microseconds, which makes the *stated* bar and the *tested* bar different things.
- **Master-width parking makes the audio a function of (log, call boundaries).** The note `.agents/notes/implemented/architecture/2026-09-05-stereo-pan-foundation.md:47-56` records the trade-off for a mid-call mixer **mount**. The unrecorded consequence is the **unmount** case: `unmount mixer @3000` inside a 96 000-frame `bounce` parks to the *end of the call*, so 93 000 frames of material the log says was silenced are still in the file — and the same log bounced at two different lengths produces two different files. The mechanism is the accepted one; the *unmount* direction and the chunk-size dependence are not written down anywhere.
- **`changes_master_width` is a heuristic keyed on the declared catalog** — `crates/engine/src/render.rs:905-932`. It parks a mount only if the plugin's `port_table` entry declares an audio Out of a different width. A plugin that claims or replaces the bus from `apply` without declaring an audio Out in its catalog (`declared == None → false`) escapes the park entirely and reintroduces the mid-call frame-count split the park was added to prevent. It also over-predicts: a mono plugin that cannot possibly take the bus is still parked when the current bus is stereo, deferring its mount by a call boundary.
- **Every node pays 32 KiB for a delay line it will never use** — `crates/engine/src/graph.rs:870-871` allocates `MAX_PDC * MAX_PDC_CHANNELS` (4096 × 2 × 4 B) per node regardless of `latency()`. A 64-channel session with 70 nodes spends 2.2 MiB on rings that are dead for every zero-latency node, and the frees land in `remove_node` on the render stack. Sizing the ring from the node's declared latency at `add_node` (and re-allocating on the control side if a rewire raises it) would make most of it disappear.
- **The plugin-registered `OpHandler` and `replay_from` are mutually unsatisfiable as documented.** `render.rs:578-588` says a handler is registered by a plugin "on mount (its `apply`)", and `arrange`'s doc (`:633-636`) repeats it; `replay_from` (`:744-746`) *fails* if the handler is not already registered. A plugin-owned handler therefore makes every log containing its ops unreplayable into a fresh engine, because the mount that would register it is itself only scheduled by the replay. The product escapes this by registering the media handlers outside the graph lifecycle (`crates/host/src/media_ops.rs:198-250`) — a contract that is nowhere written down on `replay_from`. Either move the check to apply time (park like arrangement ops) or state the ordering requirement on `replay_from`'s doc.
- **Event offsets are unvalidated platform-wide.** `EventBuf::push`/`insert_sorted` check capacity and nothing else, and `ScaleGen` copies a trigger offset straight into a `NoteEvent`. The `frames <= BLOCK` and `ch * frames <= buf.len()` slicing in `AudioInputs::get` and `render_inner` then trusts that relationship. Both findings 4 and 12 are downstream of one missing invariant: *an event's offset is within the block it was emitted in*. Declaring and enforcing that once (in `EventBuf`, at push) would close both.

### Checked and clean

Read in full, with the enclosing callers, the tests that cover them, and the two excluded files for context only:

- **`crates/engine/src/clock.rs`** — `TempoMap::{beat_at,frame_at,segment_at}` segment arithmetic, rounding and the `frame_at` overflow candidate (ruled out: `remaining < seg_beats` bounds `frames` by the segment span); `Clock::{advance,seek_to,seconds,beat,push_tempo}`; `Scheduler::{schedule,pop,peek_frame,drain_until}` ordering (FIFO within a frame via `partition_point(f <= frame)` — correct); `PatternQuery`; the three clock tests. Only the constructor-validation gap (finding 9) and the unvalidated `push` that goes with it.
- **`crates/engine/src/ctx.rs`** — `provide`/`remove`/`has`/`get` (downcast is safe, `Any` is not `Clone`-trapped); the unconditional `remove` in a disposer is a theoretical multi-provider clobber, not reachable with one instance per name.
- **`crates/engine/src/log.rs`** — the whole file. The engine owns no text codec: `Event`/`Value` are the payload and the encode/decode lives in `host`/`media`. `PartialEq` on `Value::F32` is safe here precisely because `validate_op` (`render.rs:605-611`) refuses non-finite `F32` fields, so no NaN can reach a log comparison.
- **`crates/engine/src/value.rs`** — `OpMsg::u64`'s `U32→u64` and `I64→u64` via `try_from` (no wrapping), `f32`'s strict variant match, first-match `get`. Unused, not wrong (nit 16).
- **`crates/engine/src/graph.rs`** — `EventBuf` (capacity guard, `insert_sorted`'s `copy_within` stability, documented drops); `Port::channels`/`AudioInputs::get` sizing; `add_node`/`insert_before`/`remove_node` triple-vector re-indexing (cords, `audio_out`, `audio_in_ports`, `delays` — all shifted consistently, verified by hand for both insert and remove); `connect`'s full validation ladder and its `expect` (a true invariant); `RingDelay::{set_delay,process}` including the `delay == cap` full-line case and the `delay == 0` bypass; `cumulative_latency` vs. `render_inner`'s inline `cum` (the one divergence is nit 14); `flush_frames`' reverse-topological transit, checked against the measured first-non-zero assertion in `master.rs:874-932`; `render_inner`'s clear/gather/render/PDC sequence, the `split_at_mut` output view, the `out.copy_from_slice` final copy; `MAX_PDC * MAX_PDC_CHANNELS` sizing arithmetic (`pdc ≤ 4096`, `channels ≤ 2`, `set_delay` clamps anyway); `Sine`/`Gain` (f64 phase, no wrap hazard at session lengths); `EuclideanGen`'s `b0/b1/s0/s1` window and the `frame >= block.frame && frame < block.frame + len` bound; `ScaleGen`; `ToneGen`'s blip pool, envelope and drain loop; all nine in-file tests.
- **`crates/engine/src/render.rs`** — `validate_mount`/`apply_mount`/`apply_unmount` bookkeeping; `validate_patch`'s five checks (order is the gap, finding 2); `patch`/`unmount`/`schedule_unmount`/`set_tempo`/`set_param`/`arrange`/`arrange_logged`; `apply_event`'s per-arm debug-assert discipline; `channels_for_render`'s flush-then-measure ordering; `render`/`render_into`/`render_block`'s split-and-apply loop (the `next == f1` boundary means an event stamped at a block's end frame applies at the start of the next chunk — correct, and the `written` accounting is provably exact); `render_chunk`'s clock advance; `drain`'s tail/flush re-arm, cap reporting and `MAX_DRAIN_FRAMES` clamp; `render_with_drain_aligned`'s head-trim arithmetic (sound when the documented "fully wired first" precondition holds — the width is read pre-flush, which only matters if a queued stereo mount is still pending, and the host flushes first).
- **`crates/engine/src/plugins/euclidean.rs`** — `euclid`'s `steps.max(1)` / `pulses.min(steps)` / rotation, the pattern length agreeing with the `steps.max(1)` modulus in `EuclideanGen::render` (no index panic), the four edge-case tests.
- **`crates/engine/src/plugins/scale.rs`** — degree table, the `degrees.is_empty()` guard in `ScaleGen`, the negative-count saturating cast; only `count` (finding 8) and the degree-9 fallthrough (nit 17).
- **`crates/engine/src/plugins/tone.rs`** — the `ParamDef` surface, the `blip_len.max(1)` guards on both the constructor and `set_param`, the `as u32` saturating cast on a negative.
- **`crates/engine/src/plugins/mixer.rs`** — `intern`'s once-per-name leak bound and poisoned-lock `expect`; `channel_ports`/`channel_params` vs. the nominal catalog; `MeterBank` sizing and the master slot; `MixerNode::render`'s `n = min(audio_in_count, channels)`, the `peaks` indexing, the meter store loop, `as_chunks_mut::<2>()` on an always-even `out`; `mixer_factory`'s whole-number and 1..=64 checks; the equal-power pan law; all four in-file tests.
- **`crates/engine/src/plugins/master.rs`** — `MaxWindow`'s ring (`cap + 1` slots, dominance pop, front expiry) and its sliding-max property test; `with_meters`' lookahead clamp against `MAX_PDC / 2`; `update_ballistics`; the compressor's stereo-linked detector and the limiter's read-time gain derivation (the property the brickwall rests on, with a test that pins the mid-stream ceiling drop); `dry_delay`'s same-frame reduction metering; the disposer's bus restoration; `finite_or_zero` (the sanitiser that finding 3 says belongs in the graph, not one plugin); the stereo PDC test's `first == flush_frames()` equality; the eight in-file tests.
- **`crates/engine/src/plugins/fundsp_synth.rs`** — `new`'s `cutoff.max(20.0)` / `q.clamp`, `allocate()` at construction, the per-sample stack `[f32; 1]`, the note-scan loop's `offset <= i` / `== i` structure, `remaining` decrement (cannot underflow), `latency() == 0` for a minimum-phase SVF. Only the `env_len` rescale (Design risks).
- **`crates/engine/tests/*.rs`** — all seven files, checked against the findings above to confirm none of them already covers the reported trigger.

---

## 2. Adversarial verification (verbatim)

I'll start by reading the code named in each claim.

I read the code named in each claim, plus the callers, the scheduler, the host console parser, and the tests. Verdicts:

---

### `CONFIRMED 1-engine#1`

The mechanism is exactly as described, and the error string is verbatim.

- `crates/engine/src/render.rs:224` — `if self.disposers.contains_key(name) || self.scheduled.contains(name)` is the "one instance per name" guard, and `validate_mount` is what `replay_from` calls for every logged `Mount` (`render.rs:674`).
- `crates/engine/src/render.rs:682` — the replay loop inserts into `self.scheduled` on `Mount`.
- `crates/engine/src/render.rs:684-687` — the `ScheduleUnmount` arm only calls `self.scheduler.schedule(...)`. Nothing removes from `scheduled`; only `apply_mount` (`render.rs:286`) and `apply_unmount` (`render.rs:476`) do, and neither runs during the loop.
- Live path, by contrast, does clear: `apply_unmount` → `self.scheduled.remove(name)` at `render.rs:476`, which is why the live engine accepts a remount at all.

**Trigger I used:** the log of the engine's own test `remount_reproduces_identical_signal` (`crates/engine/tests/spike_a.rs:337-353`). It calls `mount_chain` (5 mounts + 2 patches @0), then `unmount` × 4 (all logged at frame 0), then `mount_chain` again (mounts logged @96000). Walking that log through `replay_from`: events 1-7 validate clean; events 8-11 schedule `Unmount`s and leave `scheduled = {euclidean, scale, tone, mixer}`; event 12 `Mount euclidean @96000` hits `self.scheduled.contains("euclidean") == true` → `Err("plugin 'euclidean' is already mounted (one instance per name in spike A.5)")`.

Two corrections to the write-up, neither of which changes the finding: the events *before* the failure are already scheduled and already pushed into `self.log` (`render.rs:762`), so the engine is left half-populated rather than rolled back — a caller that ignores the `Err` gets a session that renders only the pre-remount prefix. And the severity label is inflated: `replay_from` has no non-test caller in the tree (only `crates/engine/tests/*`, `crates/media/tests/*`, `crates/host/tests/media_logging.rs:120`); the host's load/seek path re-issues recorded `HostCommand`s (`HostSession::is_state`/`commit_state`, `host/src/lib.rs:434-455`). No existing test replays a log containing an unmount, which is why nothing has tripped it.

---

### `CONFIRMED 1-engine#2`

Code, trigger, and both stated consequences all check out.

- `crates/engine/src/render.rs:322-384` — `validate_patch` checks plugin existence, scheduled-or-mounted, direction, kind, channel count. No index/order check anywhere.
- `crates/engine/src/graph.rs:896-900` — `if fi >= ti { return Err("connect: patch cords must go forward…") }`.
- `crates/engine/src/render.rs:412-414` — the only report is `debug_assert!(false, "scheduled patch refused at apply: {e}")`.
- `crates/host/src/lib.rs:3297-3302` — the `host v1` console accepts `mount` and `patch` in any order, with no order rule; `host/src/lib.rs:1385` forwards straight to `engine.patch`.
- `crates/engine/src/clock.rs:192-195` — `schedule` inserts at `partition_point(|f| *f <= frame)`, so same-frame events are delivered FIFO in call order. That is what fixes the node indices: the mixer's `apply` runs first (`mixer.rs:547-556` → `add_node` then `set_out`), the tone's second.

**Trigger I used:** `mount mixer channels=2 @0` / `mount tone @0` / `patch tone.audio mixer.ch0`. Mixer = node 0, tone = node 1. At apply, `graph.connect(tone=1, "audio", mixer=0, "ch0")` → `fi=1 >= ti=0` → `Err`. In release the cord never exists and nothing anywhere reports it; in a debug build `debug_assert!` panics inside `render_block` (`render.rs:1101`) on the render thread, and `crates/host/src/live.rs` has no `catch_unwind` around its pump loop (the only `catch_unwind` in the host is in `#[cfg(test)]`, `lib.rs:4031`), so the thread unwinds.

Material context the write-up does not mention: this is a *documented* trade-off, not an oversight. `.agents/notes/implemented/architecture/2026-08-17-patch-bay-typed-signal-streams.md:17` states that apply-time refusals "(forward order, single-driver) are logged-as-intent and skipped — replay reproduces the same refused state", and `apply_patch`'s own doc comment (`render.rs:386-390`) says the same. The "silent" part of the finding is therefore a chosen trade (replay determinism is preserved); the part that is *not* chosen is the debug-build panic, which contradicts that same note's "never panics on the audio path".

---

### `CONFIRMED 1-engine#3`

Every link in the chain verified, and the meters' NaN-blindness is a real `f32::max` property, not a misread.

- `crates/engine/src/graph.rs:544` — `pitch: self.root + degree as f32`; `scale_factory` (`plugins/scale.rs:87-89`) takes `root` verbatim with no bound, and `validate_mount` only rejects non-finite mount params (`render.rs:231-235`).
- `crates/engine/src/graph.rs:623` — `let freq = 440.0 * 2f32.powf(note.pitch / 12.0);` → `+inf` for `root=2000`.
- `crates/engine/src/graph.rs:637` — `inc: TAU * freq as f64 / block.sample_rate as f64` → `inf`; `:649-650` — sample 0 is `sin(0.0)=0`, then `phase += inf`, and `sin(inf)` is NaN for the remainder of the blip.
- `crates/engine/src/plugins/mixer.rs:474-475` and `:490-491` — `peak.max(g.abs())` / `self.peaks[..].max(full)`. `f32::max` returns the non-NaN operand, and `peaks` is zeroed each block (`mixer.rs:463-465`), so the peak is permanently `0.0` on a NaN bus, not NaN.
- `crates/engine/src/graph.rs:1282` — `out.copy_from_slice(&self.audio_out[i][..out.len()])`, no sanitiser; `render_chunk` (`render.rs:1172-1182`) adds none either.
- `crates/engine/src/plugins/master.rs:210-212, 392-393` — `finite_or_zero` exists only there, and nothing in `host/src/lib.rs` or `media/src` auto-mounts `master` (only the factory registration at `lib.rs:656-667`), so the default bus is the mixer.
- `crates/host/src/lib.rs:2044-2058` — export counts non-finite and refuses; `lib.rs:1684-1698` — `bounce` writes `&out` with no such check.

**Trigger I used:** the console-reachable `mount scale root=2000`. The write-up's first form, `mount scale count=1 degree0=2000`, is **not** reachable from the shipped console — `count` and `degree0` are absent from `HOST_PARAMS` (`lib.rs:71-122`), so `in_list` rejects the line — but it is accepted by `Engine::mount` directly, and `root` (the write-up's own parenthetical) is in the list, so the finding stands with that substitution.

Two overstatements: "`NaN + inf` stays NaN for the rest of the session" is per-blip, not per-session — `blip.remaining` retires the voice (`graph.rs:651-658`) and the mixer's input buffers are re-zeroed each block (`graph.rs:1166-1169`), so the bus is NaN only while a note is sounding (continuously, in practice, whenever the euclidean keeps firing). And the overflow threshold is ~1440 semitones, not ~1412 (2^119·440 ≈ 2.9e38 is still finite); irrelevant at pitch 2000.

---

### `CONFIRMED 1-engine#4`

The indexing arithmetic is correct and the reachability caveat is accurate — I checked the producer side myself rather than taking it on trust.

- `crates/engine/src/graph.rs:588-595` — `let at = self.pending_head + self.pending_count;` with only the `pending_count < MAX_PENDING` (`MAX_PENDING = 8`, `graph.rs:566`) guard.
- `crates/engine/src/graph.rs:630-644` — the drain `while self.pending_count > 0 && self.pending[self.pending_head].0 as usize <= i` advances `head` and decrements `count`, so `head + count` is invariant; `:662-664` — `head` returns to 0 only when `count` hits 0.
- `self.pending` is a fixed `[…; 8]` field, so `pending[8]` is a bounds-checked panic, not UB.

**Trigger I used (constructed, not asserted):** the test harness for a custom note producer already exists at `spike_a.rs:534-628` (a `Plugin` wrapping an `AudioNode` that pushes into the event buffer, registered via `register_factory` and patched). A node that pushes `NoteEvent{offset:0}` plus seven `NoteEvent{offset:600}` in block 0 (`BLOCK = 512`, `graph.rs:27`) leaves `head=1, count=7`; the next note in block 1 — from the euclidean or the same node — computes `at = 1 + 7 = 8` and panics. The merged buffer's capacity is no obstacle: `MERGE_CAP = 128` (`graph.rs:32`) and `EventBuf::insert_sorted` validates capacity only, never offsets (`graph.rs:96-110`).

I verified the no-shipped-producer claim: `EuclideanGen` bounds every offset by `frame < block.frame + len` where `len = out_audio.len()` = `frames` (`graph.rs:480, 495-496`; the `unwrap_or(1)` in `node_out_channels`, `graph.rs:808`, is why a trigger-only node still sees `frames`, not 0), and `ScaleGen` copies the trigger offset through (`graph.rs:543`). Every other in-tree node's note output is `_notes` (unused) — `fundsp_synth.rs:105`, `mixer.rs:453`, `master.rs:367`, `clock_out.rs:229`, `media/src/{record,capture,arranger,stream}.rs`. So this is a contract hole in `ToneGen`, not a live crash, and MAJOR overstates it.

---

### New findings

**1. `crates/engine/src/render.rs:396-411` — a patch whose endpoint is unmounted *at the patch's frame* takes the other `debug_assert` branch, and it is reachable from a saved session script.**

```rust
let Some(&from_node) = self.node_of.get(from.0) else {
    debug_assert!(
        false,
        "patch endpoint '{}' not mounted at apply (log-order error)",
        from.0
    );
    return;
};
```

`validate_patch` (`render.rs:336-341`) only asks "is the endpoint scheduled *or* mounted **now**" — it cannot know the endpoint will be unmounted before the patch's frame. The reviewer's proposed order-check fix would not cover this branch.

**Trigger** (a valid `host v1` script — `run_script`/`from_script`, `host/src/lib.rs:2316-2333`, executes each line through `process`, which pre-renders to `@frame` at `lib.rs:2716-2723`):

```
host v1
mount mixer channels=2 @0
mount tone @0
unmount tone @100
patch tone.audio mixer.ch0 @100
```

Line 3 renders to frame 100 (applying both mounts), then schedules `Unmount{tone}` @100 while `node_of` still holds `tone`. Line 4 sees the clock already at 100, so no pre-render; `engine.patch` validates against the still-mounted instance and logs `Patch` @100. On the next render, the scheduler (`clock.rs:192-195`, FIFO within a frame) delivers the `Unmount` first, so `node_of.get("tone")` is `None` and the tripwire fires.

**Consequence:** identical shape to claim #2 — `debug_assert!` unwinds the render block on the pump thread in debug (`live.rs` has no `catch_unwind`); in release the cord is silently skipped. Because this survives a `save`/`open` round-trip, a session that once played correctly re-loads into a silently mis-wired state.

**2. `crates/engine/src/plugins/mixer.rs:571-580` vs `crates/host/src/lib.rs:71-122` — a mixer mounted wider than 8 channels exposes a parameter surface the console cannot address.**

`MIXER_CHANNELS_SANITY = 64` and `mounted_ports`/`mounted_params` generate `ch0..ch63`, but `HOST_PARAMS` stops at `ch7.gain/mute/solo/pan`. A `mount mixer channels=12` is accepted (`lib.rs:2714-2720` bounds only at 64), the TUI's key-driven `set_param` path works (it builds `HostCommand::SetParam` directly), but the same operator typing `set_param mixer ch9.gain 0.5` into the `:` line gets `unknown param 'ch9.gain' (registry: …)` from `in_list` — a parameter the UI just offered. Minor, cosmetic, but it is a registry that disagrees with itself.
