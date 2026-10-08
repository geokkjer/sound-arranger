# Agent Note: A generative composer profile with an independent CLAP host

Status: proposed

## Problem

The browser generative composer has useful musical behaviour, but its FM-1 origin constrains the
model to four tracks, four project slots, device-specific destinations, and a small sequencer grid.
The next instrument should compose on a desktop without inheriting those limits or rebuilding sound
engines and recording infrastructure that already exist elsewhere.

> Last verified against sound-arranger commit `8b2ebdf` (2026-10-08). The browser source is an
> uncommitted working-tree snapshot inspected on the same date; its model checks pass. This is a
> plan, not a claim that the composer, CLAP hosting, or process bridge already works.

The source is [the browser model](../../../../../music/music-composition-theory/studio/instruments/m-vave-fm-1/web/fm1gen.js)
and its [design and tests](../../../../../music/music-composition-theory/studio/instruments/m-vave-fm-1/web/README.md).
The musical goal is **a pure generative composer**: authored processes produce musical events;
CLAP instruments make sound; this platform's recorder captures the performance.

### Decisions established with the owner

- Evolve the browser composer, rather than make an exact Rust transcription. Remove hardware limits.
- Keep CLAP hosting an **optional capability — a plugin in the composer profile's mount set**, not a
  platform capability — maintained as a **separate project**. Do not put CLAP SDK dependencies or
  plugin-format knowledge in the minimal engine core.
- Keep **in-process library versus external worker** open until an early comparison. A separate
  repository and a separate operating-system process are different decisions.
- Deliver a **finite piece first**, with endless generation and live steering as later extensions.
- Prioritize **experimental systems**: independent cycles, phase relationships, bounded chaos,
  interaction, changing density, and composed long-term development.
- Use **Surge XT** as the first real-instrument compatibility target. The owner proposes **Cardinal**
  before eventual VCV Rack integration; the modular-stage scope remains to be agreed.

**Clarified with the owner (2026-10-08, after the first review):**

- **The composer is a profile**, and its generators are the platform's existing plugins: `euclidean`
  and `scale` (already registered in `HOST_PLUGINS`) **fold into the composer profile's mount set**
  rather than being reimplemented here. `crates/composer/` holds the composition layer — spec,
  compiler, plan, audit — not a second generator registry.
- **The CLAP host is a plugin in that mount set** — not a profile and not a platform capability.
  `tone` stays mounted as the **diagnostic voice**, so the event → sound → take path is testable with
  no third-party plugin installed; the hosted instrument is the product voice. A composer profile
  must open and report a missing instrument rather than depend on one being present.
- **Profiles stay coarse** (a plugin is the small unit; a profile is a program; a rig wires
  programs) and **the rig/studio is the end goal** — see
  [the profiles note](2026-09-27-profiles-and-the-umbrella-name.md).
- **The detailed model is deliberately open.** The specification shape, the part/cycle/route
  semantics, and the operator set are not decided. The implementation plan below is a **sketch to be
  re-cut as the shape becomes clear**, not a committed sequence. What is committed is the direction:
  authored processes → events → an instrument → the existing recorder.

The [external-programs decision](2026-09-21-external-programs-not-sidecars.md) rejects in-process
third-party hosting. This proposal reopens that boundary narrowly for an independently maintained,
optional CLAP host; if the in-process option wins, a follow-up decision must explicitly supersede
that prohibition. An external worker stays closer to the existing integration policy. Neither
choice authorizes VST3/LV2/AU hosting or moves synthesis into the core.

## Proposal

Add a **composer profile** to the [audio platform's profile model](2026-09-27-profiles-and-the-umbrella-name.md).
The profile will assemble composer plugins, an optional CLAP-host integration, existing routing and
mastering, and existing recording. It will not be another DAW, synthesizer collection, or recorder.

### Ownership and boundaries

```text
Versioned composition specification + seed
                |
       Composer plugins
       generators / parts / cells / route / transitions
                |
       Compiled performance plan
       notes + controls + timing + decision trace + musical end
                |
       Platform event players and CLAP integration plugin
                |
       Independent CLAP-host project
       library OR worker, selected after the spike
                |
       Existing audio graph / mix / master
                |
       Existing recorder -> pool takes (declared state; provenance is not a model yet)
```

**The platform owns time.** Compositional positions can be expressed as beat fractions and local
cycles, but the existing `TempoMap` converts them to session frames. Logged actions retain the
platform's frame-based semantics. Neither a generator nor the CLAP host gets an autonomous transport.

**Composition and performance state are separate.** A specification describes the rules; a compiled
plan describes one finite realization. Plugin identities, sound states, parameter mappings, and
capture placement describe how that realization is performed. Keep all three as versioned session
assets, referenced by the existing log rather than replacing it with a second journal.

**Everything is a plugin, not everything is a dynamic library.** The composer capabilities use the
platform's existing Rust `Plugin`/`AudioNode` discipline. The third-party instruments are CLAP
binaries behind the host adapter. Initially, new generator implementations can be compiled Rust
modules with a small registration surface; no new Rust dynamic ABI or scripting language is needed.

The rhythm and pitch generators the composer needs **already exist as engine plugins** —
`euclidean` and `scale`, registered in `HOST_PLUGINS` — so the profile mounts them and shares their
pure functions (`euclid`, the scale helpers) instead of growing a parallel implementation. Their
known limits are real design inputs: `scale` is a single `forward` counter over a privately-owned
degree vector, which is exactly what the
[decoupled-pitch-rhythm note](../feature/2026-08-20-decoupled-pitch-rhythm-generators.md) was
written to fix. That note, and the [musical-event model](2026-08-15-musical-event-model.md), own the
composer's pitch/rhythm design; this note owns only the assembly and the chain to the recorder.

Proposed ownership, with names provisional:

| Location | Responsibility |
|---|---|
| `crates/composer/` | Optional composition capability: typed specification, generators, part state, route compiler, audit, platform registration. No CLAP or audio-device dependency. |
| Separate CLAP-host project | CLAP discovery, lifecycle, event conversion, processing, state, and eventually plugin editors. Exposes a library API; a worker executable is added only if selected. No composition or recording code. |
| `crates/host/` integration module | Profile assembly, prepared host instances, session assets, commands, and recording lifecycle. Depends on the external host only when enabled. |
| `crates/media/` | Existing capture, pool, WAV, peaks, and take machinery; only small shared improvements when a real integration needs them. |
| Existing TUI/iced shells | Later composer views over the same host commands. Headless operation comes first. |

Do not create the separate repository, publish crates, rename the platform, or add dependencies as
part of this planning change. Choose the project's name and licence before its first implementation.

### Cargo libraries: verified shortlist

Registry metadata, versioned API docs, and upstream source were inspected on 2026-10-08. These are
**available candidates**, not locally compiled or plugin-compatibility-tested dependencies.

| Candidate | What exists | Assessment |
|---|---|---|
| `clack-host` **0.2.0** + `clack-extensions` **0.2.0** | Safe, low-level CLAP wrappers; dynamic loading, lifecycle/thread separation, audio/event processing, and extension APIs. MIT OR Apache-2.0; registry MSRV 1.85. | **First candidate.** CLAP-only and explicit about the lifecycle. We still implement the host callbacks, registry, buffering, integration, and failure policy. |
| `truce-rack-clap` **1.1.5** | Higher-level `ClapScanner` and `ClapPlugin`, built on `clap-sys`; scan/load/process APIs. MIT OR Apache-2.0; registry MSRV 1.90. | **Compare in the spike.** Use the CLAP-only crate, or disable the umbrella's default VST3 feature. Audit realtime behaviour, parameter timing, thread ownership, state, and Linux support. |
| `clap-sys` **0.5.0** | Raw CLAP FFI bindings. | A foundation, not a usable host by itself. Prefer not to write another unsafe wrapper layer. |
| `rack` **0.4.8** | Original host library; its upstream README still labels CLAP support as planned. | Not the CLAP solution. Do not confuse it with the separate `truce-rack` rewrite. |
| `plugin_host` **0.1.0** | Published API describes hosting and sandboxing, but its CLAP scanner returns an error, processing is a stub, and the sandbox handle does not own a child process. | Exclude from the implementation shortlist. API names are not evidence of working hosting. |

Clack does not provide process isolation. Truce's standalone runner is useful prior art, but its
upstream README disables the Linux GUI path; do not assume that runner meets our editor needs.
`nice-plug`/`nih-plug` build plugins, not hosts. Cargo's `clap` crate parses command-line arguments;
it is unrelated to the CLAP audio format.

### Instrument milestones: Surge XT, then Cardinal, then optional VCV Rack

These musical milestones are distinct from the implementation phase numbers below. **Surge XT is
the first real instrument; Cardinal is the proposed second-stage modular target**, not a prerequisite
for proving the composer. No instrument is installed or bundled by this planning change.

| Milestone | Target and purpose | Scope boundary |
|---|---|---|
| **First usable composer** | **Surge XT CLAP**: two or more instances with different saved sounds, receiving the Rust composer's parts. | Notes/releases, stereo, transport, state recall, and a few explicitly mapped controls. Do not embed or port its DSP. |
| **Modular stage** | **Cardinal Synth CLAP**, then Cardinal FX: reusable modular instruments, macro-controlled textures, and later audio processing. | First prove the selected CLAP build and fixed patch. Do not assume CV ports, arbitrary module discovery, or autonomous-patch reproducibility. |
| **Optional later integration** | **VCV Rack Pro CLAP** when specific Rack-library or commercial modules justify it; alternatively Rack Free as an external program. | Reuse the host contract for Pro. The Free path needs a real MIDI/audio/sync bridge and is not an already-working feature. Neither path is mandatory. |

**Why Surge first.** Its published CLAP documentation describes sample-accurate automation, note
expressions, and mono/polyphonic modulation, making it useful beyond the simplest note test. Its
synthesis/modulation range is broad enough for several contrasting parts without adding another
plugin immediately. Begin with a small, inspectable saved patch, then add complex patches only after
timing and releases are proven. Use a tiny test plugin alongside Surge so a sophisticated preset
does not hide a host defect. Verify the actual installed build's capabilities rather than treat an
old feature announcement as a compatibility test.

**Cardinal is a good second host implementation test.** Surge and Cardinal use different plugin
frameworks; success with both is stronger evidence than two presets in one synth. Cardinal is
self-contained with a fixed, built-in module collection; it does not install modules from the VCV
store. Its included module roster also lists Surge XT modules, but that does not imply identical
plugin parameter IDs or automatic Surge-preset translation.

The Cardinal integration must respect these documented limits, verified on 2026-10-08:

- Its upstream README still labels **CLAP support as work-in-progress**. Test the selected binary
  before committing the modular stage; a format appearing on the download page is not a passing test.
- **Cardinal Synth** exposes stereo audio output, with no audio input or CV ports. **Cardinal FX**
  adds stereo audio input/output. The multi-I/O **Main** variant is documented as unavailable in CLAP;
  its current Makefile's `clap` target builds FX and Synth only. Do not promise raw CV over CLAP.
- Patch control uses **24 host-exposed parameters** through Host Parameters/Map modules. Map a small
  subset to named musical macros and discover their actual CLAP IDs at runtime. Module knobs are not
  all independently exposed as plugin parameters; UI/system parameters must not be mistaken for patch
  macros merely because they appear in the parameter list.
- **Host Time** provides patch-facing transport information. Define how a patch reacts to play,
  reset, and tempo changes; do not add a second unsynchronized clock inside it by default.
- Cardinal's **OSC remote control is standalone-only**, not available in the plugin variant. CLAP
  control uses plugin events/state; standalone OSC is a different future integration.
- Save the full plugin patch state and dependency references, not only the 24 macro values. A fixed
  module collection does not make a patch's external sample files self-contained.

**VCV clarification.** Hosting Rack as CLAP is included in the paid **Rack Pro** product, alongside
its other plugin formats. The separately purchased **VCV Host** module is the opposite direction:
hosting other plugins *inside* Rack. There is no need to buy it to host Rack Pro in our program.
Rack Free remains a possible standalone partner without paying for Pro; its process/device bridge
would still need implementation and measurement.

### Modular composition ownership

Keep two explicit usage modes rather than quietly moving the composer into a patch:

1. **Instrument mode (recommended first):** Rust owns notes, rhythms, form, and musical ending;
   Cardinal turns notes and mapped macros into sound. Its internal envelopes/LFOs are sound design,
   with transport/retrigger behaviour documented in the instrument contract.
2. **Process-instrument mode (later):** Rust owns the long-form arc and sends transport plus macro
   trajectories; a modular patch generates local rhythms/pitches. This is a collaborator/process
   instrument, not merely a replaceable synth voice. State and reset policy must be explicit, and
   unseeded/free-running modules prevent promises of exact audio regeneration.

For the first Cardinal patch, avoid nested plugin hosts, autonomous sequencers, and uncontrolled
random sources. Publish a reusable instrument contract: note input, output channels, macro meanings
and ranges, reset/retrigger rules, assets, version identity, and tail behaviour. Suggested musical
macros are brightness, texture, resonance/feedback, modulation depth, and space; activity/density
becomes a patch macro only when the patch intentionally owns that process.

Design the host generically: Surge and Cardinal test it; neither should dictate its event schema.
Keep advanced note-expression/polyphonic modulation as a later capability test, not a reason to
expand the first release before basic recording works.

### What to preserve, improve, and leave behind

| Preserve | Improve | Leave behind |
|---|---|---|
| Seeded, bounded generators; Euclidean rhythms; rotate/subset/invert; independent drum lanes; swing versus drift; cells and routes; run-twice audit. | Stable identities, flexible part counts and cycles, explicit reset rules, bounded route graphs, real transitions, richer musical events, neutral modulation mappings, complete endings. | Four-slot alphabet as a structural limit, 16-step-only authoring, shared eight-voice budget, fixed GM kit, device SysEx/flash writes, two-operator preview synth. |

Hardware compatibility can remain a future exporter, not the composer's data model. Keep the browser
prototype intact as a reference while the Rust implementation develops.

**Operator rollout:** Euclidean rhythm and WALK establish the first path. Add LFO/sample-and-hold,
then CHAOS, then PHYSICS as separate operator-and-test slices after Task 5; each depends on Task 4's
state/numerical contract and must pass boundedness, seeded replay, and trace tests. These operators
remain part of the intended evolution, but a complete physics port does not gate the first usable
release. Rotation/subset/inversion can be characterized alongside the discrete rhythm fixtures.

For experimental music, harmonic rules must be **optional policies**, not hidden defaults. Do not
force every strong beat onto a chord, every line into a diatonic scale, or every pair of parts away
from unison. Pitch collections, register, leap limits, doubling, and tuning are intentional choices.

### Improvements that matter musically

1. **Compose at multiple timescales.** Operators generate gestures; parts maintain identity;
   section processes shape density, register, rhythmic activity, and timbral development. A finite
   piece needs an arc, not just a loop that runs until a timer expires.
2. **Independent cycles before more randomness.** Let one part repeat five steps and another seven,
   with explicit rates and phase. Rotation, gradual phase displacement, and scheduled mutation should
   change relationships without erasing the material's identity.
3. **Relationships before isolated streams.** Add bounded rules such as answering another part,
   avoiding its recent onset, or increasing activity when another part rests. Read prior-step
   summaries initially, so feedback is causal and cannot create an instantaneous evaluation loop.
4. **Transitions are authored processes.** Interpolate declared continuous controls; schedule discrete
   rhythm/pitch changes at declared boundaries. Do not interpolate arbitrary plugin state blobs or
   switch instrument presets while notes sound and call it a morph.
5. **Name the controls musically.** Density, activity, phase spread, register width, persistence, and
   transition length are useful macro controls. Map them explicitly to selected plugin parameter
   IDs/ranges; no universal assumption that every synth has a parameter called `filter` or `shape`.
6. **Make the result inspectable.** Retain generator traces, route choices, mutations, constraint
   conflicts, and event-rate estimates. Explain an unexpected result without pretending an audit can
   establish artistic quality. Listen to fixed reference pieces after each musical slice.

The [Composer's Onion](../../../../../music/music-composition-theory/theory/framework/the-composers-onion.md)
locates this primarily in Layer 7: authored rules shape the whole piece. CLAP delegates sound
generation; recording preserves a realization. “Generative” here does not require AI or ML.

### Correctness and realtime contracts

- **Stable seed domains.** Derive streams from the composition seed plus explicit cell, part, lane,
  and operator IDs using a specified, versioned mixing function. Do not use Rust's default hash or
  array order as musical identity. Adding an unrelated part must not change another stream's draws;
  an explicitly coupled part may change its response, which the trace should explain.
- **Explicit state lifetime.** A cell entry declares reset, resume, or continue for each process.
  Distinguish route-entry boundaries from a cell's identity. Repeating the same cell can either
  restart or continue; that is a musical choice, not an equality-check side effect.
- **A determinism promise with a boundary.** The same specification, seed, engine version, target,
  and realization mode must produce the same canonical event plan. Floating-point chaos and
  transcendental functions do not justify cross-platform bit-equality promises. Persist the compiled
  plan for exact event replay across upgrades; test cross-target guarantees before advertising them.
- **Audio is a separate promise.** Third-party plugins can use internal randomness, free-running
  state, or different offline modes. Audit musical events independently from audio reproducibility.
  Save plugin state/version identity, but the recorded take remains the authoritative sonic artifact.
- **One event contract.** Define onset, duration, target, stable note identity, velocity, and control
  events independently of CLAP. Convert MIDI note 69 to the existing engine's pitch 0 at the adapter.
  Negotiate supported note dialects; do not send the same onset in both CLAP and MIDI encodings.
- **No lost releases.** The adapter tracks pending note-offs across blocks, overlap on the same key,
  stop, seek, reset, and end-of-piece. Note release and voice termination are different. Preserve
  release/effect tails with a bounded drain; report truncation if the declared maximum is reached.
- **No hidden event loss.** The current graph has a 32-slot out buffer **per node**
  (`CAP_EVENTS = 32`) and a 128-slot merged fan-in (`MERGE_CAP = 128`); a node that merges many cords
  still re-emits through its own 32. Preflight **both** numbers, report overflow, and reject an
  unsafe dense plan before play. A later generic buffer improvement is justified by measurements,
  not an unchecked capacity bump.
- **Prepare off the render path.** CLAP loading, instantiation, activation, state I/O, callbacks, and
  GUI work follow the appropriate lifecycle/thread rules. The current `Plugin::apply` runs on the
  render stack, so it **must not** perform these operations — a contract enforced by the render-path
  tests, not by the type signature. Publish prepared processing handles through a control-side seam;
  retire them off-thread as well.
- **Bound every operation.** Compile and allocate off the audio thread. Audio callbacks do not wait
  for IPC, allocate, lock, scan directories, or write files. External native plugin code cannot be
  made safe merely by wrapping it in Rust; a worker provides crash isolation, not a security sandbox.

### Integration gaps already visible in the code

These are planning inputs, not requests to fix the prototype in this session.

- `fm1gen.js` stores `morph` but its renderer does not apply it. The route is a linear list, not the
  proposed graph. The Rust plan must implement these behaviours rather than claim to port them.
- Drum seed derivation includes lane and bar but not part/cell identity, allowing unintended
  correlation. Mutation also runs after a lane is muted, so it can add a hit back. Add invariants for
  stream independence and “muted means no generated hits.”
- The current graph exposes one note-output port per node. Use one playback node per part first,
  rather than assuming a single composer node can expose arbitrarily many note lanes.
- Plugin factories and mounted identities currently share a name, with one active instance per
  name. Multi-part hosting needs a small generic factory/instance distinction, not `clap1` through
  `clap64` hard-coded registrations.
- The engine's `NoteEvent` carries pitch, velocity, duration, and offset, but not arbitrary timed
  controls or rich note addressing. The initial note-only adapter can synthesize releases; richer
  events require an explicit extension or plugin-owned plan seam, not a block-rate scalar shortcut.
- Stereo CLAP output does not fit the existing mono mixer inputs directly. Start with explicit
  channel extraction/routing into two existing mixer channels, preserving L/R. Expand bus handling
  only when the integration proves that a generic stereo-input capability is needed.
- `HostSession::start_recording` already accepts an interleaved `Spsc<f32>` ring and uses
  `media::Capture`, and equal-rate capture passes through. **No host-level graph tap exists today**:
  the record path feeds `Capture` from a device or a caller-owned ring, `RecordNode` is on the older
  mono `Recorder` seam, and `CaptureNode` is the capture→monitor direction. The tap is new wiring,
  not a hook. The source-declaration model's JACK/OSC names are not implemented audio-process
  bridges either.
- The host currently allows one active capture. A first stereo mix take fits that model; simultaneous
  per-part stems need a later extension. Monitor-ring drops, source-ring overruns, and worker misses
  are different counters and must not be conflated.

## Implementation plan

Each task is a small vertical slice. Paths below are proposed ownership locations, not existing
files promised by this note. Keep each slice to roughly 1–5 implementation/test files; split it
again if the prototype exposes a larger change. No implementation starts before plan approval.

**This plan is a sketch, not a commitment.** The owner wants the composer; the shape of it is not
settled. The direction and the boundaries above are what this note commits to — the task list is
re-cut as the shape becomes clear, and it is not ordered work yet.

### Phase 1: Contracts and the high-risk host seam

#### Task 1 — Specify one finite realization

Write the smallest behavioural contract: seed and rules produce events with a musical end; an
instrument performs them; the recorder preserves the result. Specify reset, overflow, stop, and tail
behaviour before choosing serialization syntax or UI.

- [ ] Scope separates composition, host, audio clock, and recording, with no new synthesis engine.
- [ ] One experimental reference piece has measurable timing/duration and a listening brief.
- [ ] Plugin selection, editor requirements, and recording tap are decided or explicitly open.

**Verification:** Trace a short piece from start through finalization, including missing plugin and
early stop. Review the contract with the owner. **Dependencies:** None. **Scope:** Small; this note
and one contract document.

#### Task 2 — Compare the two Cargo host candidates

In an isolated spike belonging to the separate host project, load the same test instrument through
Clack and Truce. Drive a fixed event list without any composition code or audio device requirement.

- [ ] Both candidates compile, or their concrete blockers are recorded; no README-only verdict.
- [ ] Tests exercise notes/releases, stereo output, transport, one timed parameter, and state reload.
- [ ] Record missing callbacks, allocations under our control, Linux limitations, and dependency cost.

**Verification:** Run a deterministic test plugin plus Surge XT CLAP; compare event handling and audio
onset placement. Record the tested Surge build and state fixture. **Dependencies:** 1. **Scope:** Medium; separate host project
`spikes/host-probe/` and a findings note. Do not install system-wide dependencies as part of the spike.

#### Task 3 — Compare deployment boundaries

Use the winning library in two small experiments: direct processing and a minimal worker. The worker
must receive frame-tagged events and return frame-tagged audio; untimestamped MIDI over a pipe is not
an equivalent implementation. Choose one production path, not two permanent backends by default.

- [ ] Measure end-to-end latency, CPU, deadline misses, and alignment over a ten-minute realtime run.
- [ ] Worker timeout/termination leaves the platform responsive and the partial take recoverable.
- [ ] Record the chosen process scope, buffering latency, failure policy, and portability cost.

**Verification:** Use the same impulse/event fixture in both paths, interrupt the worker, and inspect
capture placement. Never block the platform audio callback waiting for a response. **Dependencies:**
2. **Scope:** Medium; separate host project probe/bridge files and a decision note.

**Checkpoint:** Choose the library and deployment model. If Linux compatibility or realtime exchange
fails, stop and revise the boundary; do not build composition features around an unproven host.

### Phase 2: A composer that already records, before adding breadth

#### Task 4 — Generate one bounded part

Add a versioned typed specification and compiler with one explicit part ID, Euclidean rhythm, seeded
walk, duration, and ordered events. Import a narrow subset of browser examples as characterization
fixtures; preserve useful semantics, not incidental JavaScript behaviour.

- [ ] Repeated compilation produces the same plan; invalid durations/ranges are refused by name.
- [ ] PRNG and discrete rhythm/variation fixtures agree with their documented reference behaviour.
- [ ] Generator state cannot depend on wall time, iteration order, or a shared ambient RNG.

**Verification:** `cargo test -p composer`; property tests for bounds, ordering, finite output, and
seed isolation. Compare floating traces only within a stated numerical policy. **Dependencies:** 1.
**Scope:** Medium; `crates/composer/{Cargo.toml,src/lib.rs,src/spec.rs,src/compile.rs,tests/basic.rs}`.

#### Task 5 — Play and record that part through existing infrastructure

Mount a part-player plugin, use the existing tone solely as a diagnostic instrument, and feed an
internal audio tap into the existing recorder. This proves the whole event → sound → take path before
third-party plugin behaviour obscures a timing bug.

- [ ] A finite plan plays from session frame zero and yields a declared take with the expected length.
- [ ] The producer is quiescent before recorder finalization; the source ring has no overruns.
- [ ] The saved take reopens without an input device or rerunning the composer.

**Verification:** Headless integration test with known onsets and channel lengths; verify save/load.
Pace realtime capture; do not flood the ring with an unbounded faster-than-realtime render.
**Dependencies:** 4. **Scope:** Medium; composer player, host tap/session wiring, integration tests.

#### Task 6 — Separate plugin type from instance identity

Add only the generic identity distinction required to mount two part players and two instrument
adapters. Keep old single-instance scripts readable and avoid expanding the core with composer terms.

- [ ] Two instances of one factory can be patched, addressed, and unmounted independently.
- [ ] Existing scripts retain their meaning; duplicate instance IDs fail atomically.
- [ ] Replay and undo reconstruct the same instance graph and dispose every contribution.

**Verification:** Engine lifecycle tests plus host codec/replay tests. **Dependencies:** 5.
**Scope:** Medium; engine mount/log/registry, host codec, focused tests. Split the log migration into
its own slice if backward compatibility requires more than these changes.

**Checkpoint:** One generated part records end to end, and repeated plugin instances are reliable.
The host-library spike and Tasks 4–6 can proceed independently once Task 1's contract is stable.

### Phase 3: Finite experimental form and real CLAP sound

#### Task 7 — Independent parts and cycles

Add flexible parts, drum/event lanes, pitched and sustained material, rational cycle rates, rotation,
and phase. Keep harmony/register rules opt-in. Use declared resource bounds rather than FM-1 limits.

- [ ] A five-step part and seven-step part retain independent phase across section boundaries.
- [ ] Reordering or adding an uncoupled part does not perturb existing streams.
- [ ] Muted lanes remain silent even when mutation is scheduled; note duration is independent of grid.

**Verification:** Property tests and a short three-part reference performance; listen for the changing
relationship rather than judge only note validity. **Dependencies:** 4, 6. **Scope:** Medium;
composer part/cycle/lane modules and tests.

#### Task 8 — Cells, bounded routes, and an actual ending

Compile a linear route first, then add a small graph with seeded choices, visit limits, and terminal
conditions. Make process reset/resume/continue explicit. Shape an expansion and contraction in
activity; a finite experimental piece need not use verse/chorus form.

- [ ] Cyclic routes cannot hang compilation: duration/visit bounds force a defined termination policy.
- [ ] The trace explains cell entry, branch choice, state policy, and final note releases.
- [ ] A reference realization develops over time and ends intentionally, without stranded notes.

**Verification:** Route-cycle/terminal tests and a fixed-seed three-minute listening example, with
assertions on density windows and the musical end. **Dependencies:** 7. **Scope:** Medium; route,
cell-state, form-envelope modules and tests.

#### Task 9 — Replace the diagnostic voice with hosted instruments

Add the selected external host as an optional platform integration plugin. Keep lifecycle preparation
off-thread, adapt note durations and pitch units, and preserve stereo through the existing mixer and
capture. The external host must still work independently of the composer.

- [ ] Two hosted instances perform different parts with correct offsets, releases, and stereo routing.
- [ ] The musical end drains releases/effects to a bounded tail, with truncation reported if necessary.
- [ ] Missing plugin, activation refusal, or selected worker failure yields an explicit outcome.

**Verification:** Adapter tests with a test CLAP plugin; two Surge XT instances with distinct saved
states; waveform onset/placement checks against the diagnostic path. **Dependencies:** 2, 3, 5, 6.
**Scope:** Medium; optional host adapter, prepared-instance seam, tests. Treat the thread-lifecycle
seam as a prerequisite slice if it cannot fit here.

#### Task 10 — Save the composition and the sound realization

Store the specification, compiled plan, algorithm/schema versions, decision trace, instrument IDs,
plugin-state blobs and hashes as session assets. The existing log references those assets and records
the finished take. A file path alone is not a reproducible sound state.

- [ ] Save/load replays the exact event plan and restores supported sound states without regenerating.
- [ ] Playback of the recorded take works when its original CLAP instrument is absent.
- [ ] Missing assets or changed plugin identity are reported; no silent substitute or state discard.

**Verification:** Round-trip session test, missing-plugin test, modified-asset test, and existing
recorder replay regression tests. **Dependencies:** 8, 9. **Scope:** Medium; composer asset codec,
host session integration, tests.

**Checkpoint / first usable release:** Seed + rules produce a finite experimental piece through real
CLAP instruments; the existing recorder captures it; the session and take survive reopening. The
model is not frozen here: this release proves the **chain** (spec → events → hosted sound → take →
reopen), and the compositional depth above it is expected to keep moving.

### Phase 4: Compositional depth, then the surface

#### Task 11 — Parameter mappings and real transitions

Discover parameter IDs/capabilities, bind explicit mappings, and add timed control events and
continuous transition curves. Resolve the richer-event seam openly: use a minimal generic timed-event
extension or an adapter plan interface, not private CLAP vocabulary embedded in the core.

- [ ] A generator modulates a chosen parameter within its declared range at a stated control rate.
- [ ] A transition distinguishes continuous controls from discrete changes and preserves note releases.
- [ ] Unsupported mappings fail visibly; parameter labels and array indices are not durable identity.

**Verification:** Test-plugin parameter trace, event-capacity tests, and a listened transition fixture.
**Dependencies:** 9, 10. **Scope:** Medium per slice; separate discovery/mapping from timed-event
plumbing if both require independent API changes.

#### Task 12 — One explicit inter-part process

Add a single coupling rule, such as one part answering another's preceding activity. Make the rule
and its timescale inspectable, then evaluate whether it creates a stronger piece before adding more.

- [ ] Evaluation is causal and bounded; no same-tick feedback loop or part-order dependence.
- [ ] The trace explains when the coupling changes an onset or activity decision.
- [ ] An A/B reference realization isolates the musical effect with the same underlying seeds.

**Verification:** Causality/invariance tests and owner listening review. **Dependencies:** 7, 10.
**Scope:** Small; one relationship operator, trace support, tests/reference fixture.

#### Task 13 — Headless commands and a small composer view

Expose compile, audit, play, record, inspect, and stop through the host command vocabulary. Add a
minimal TUI view for parts, process parameters, form position, and capture status only after the
headless workflow is stable. Do not reuse the clip timeline as the composition model.

- [ ] One documented headless workflow realizes and records a saved specification.
- [ ] Errors expose their owning subsystem; audit results distinguish events, traces, and audio.
- [ ] The initial view reports the same state as the CLI without introducing a second scheduler.

**Verification:** Command parser/replay tests; hardware-free shell snapshot test; manual first-piece
procedure. **Dependencies:** 10; expose Tasks 11–12 only when implemented. **Scope:** Medium per
slice; CLI/host commands first, TUI separately, iced later.

**Checkpoint:** Listen to the reference piece again. Improve musical control before adding generator
count, GUI breadth, a live-coding language, or another plugin format.

### Proposed modular follow-on: Cardinal, before VCV Rack

#### Task 14 — Qualify Cardinal Synth as another instrument

Use one externally authored, fixed Cardinal Synth patch with note input, stereo output, and explicit
Host Parameters mappings. Start in instrument mode; the Rust composer remains responsible for notes
and form. Patch-authoring UI and embedding that UI in our shell are separate requirements.

- [ ] The selected CLAP binary loads, follows transport/reset, handles note releases, and reloads state.
- [ ] At least three named patch macros respond through discovered IDs with declared ranges.
- [ ] The same compiled composition records through Surge or Cardinal without composer changes.

**Verification:** Fixed-state note/transport/macro tests, dependency-manifest round trip, and a
ten-minute capture smoke test. Treat CLAP failures as measured blockers, not an excuse to add more
plugin formats silently. **Dependencies:** 10, 11. **Scope:** Medium; instrument-contract fixture,
host compatibility tests, and results note. The musical patch is an external fixture asset, not new
Cardinal implementation code in this repository.

#### Task 15 — Qualify Cardinal FX as a processing node

Only after Synth works, exercise audio-input handling with a fixed stereo effect patch. This tests a
capability Surge-as-an-instrument does not prove: processing audio through another CLAP instance.

- [ ] Stereo channels remain distinct through FX, mixer/master, and the recording tap.
- [ ] State recall, silence, feedback bounds, and effect-tail termination have tested outcomes.
- [ ] Processing requires no nested host or special Cardinal scheduling path.

**Verification:** Impulse/channel-routing tests and a listened transition between dry/wet states.
**Dependencies:** 14. **Scope:** Medium; generic host audio-input wiring if needed, tests, and fixture.

**Checkpoint:** Decide whether a process-instrument patch adds musical value. Add VCV Rack only when
a required module or workflow is missing from Cardinal; do not make it the mandatory next migration.

## Alternatives considered

- **Exact browser port.** Rejected as the product direction: the owner wants evolution on less
  constrained hardware. Preserve selected characterization fixtures, not device ceilings or bugs.
- **CLAP hosting inside the engine crate.** Rejected: plugin SDK/thread/editor dependencies violate
  the minimal core. The independently maintained host is reached through a platform plugin.
- **External worker immediately, without comparison.** Not selected yet: useful crash isolation,
  but realtime exchange, buffering, and alignment are real work. Task 3 measures them.
- **Separate library immediately, without comparison.** Not selected yet: direct routing is simpler,
  but an invalid native plugin can crash the whole application and conflicts with the prior policy.
- **Build both production paths now.** Rejected: measure both, ship one. A second backend needs a
  demonstrated requirement, not a speculative abstraction budget.
- **New synthesizer or WAV recorder.** Rejected: CLAP supplies sound and the platform already owns
  capture, pool, mastering, and takes. The existing tone remains a test instrument only.
- **Use `plugin_host` because it advertises sandboxing.** Rejected after inspecting its published
  source: the relevant backend and process execution are placeholders.
- **Endless generation or live coding first.** Deferred by the owner's finite-piece choice. Those
  modes require checkpointing, safe edits, and persistent decisions; they follow a working bounded
  realization rather than replace its finish line.
- **Use an existing generative program (TidalCycles) and record it.** The
  [external-programs decision](2026-09-21-external-programs-not-sidecars.md) names Tidal as the
  generative partner. Not selected for the composer — the authored-process vocabulary and the
  run-twice audit are the point, and they need our own event model — but kept on the table: the
  recorder path already supports it, and it stays the cheapest way to exercise a rig before the
  composer exists.
- **Drive the owner's hardware by MIDI instead of hosting CLAP.** The platform already sends MIDI
  clock, and the browser model's own roadmap points at the FM-1. Not the product voice, but a
  compatible early path and a useful smoke test of the chain; keep it in view once the host lands
  rather than letting the hardware work become an accidental exporter-only afterthought.
- **Split the mixer/master into profiles of their own.** Rejected: the small unit is the plugin and
  profiles stay coarse ([profiles note](2026-09-27-profiles-and-the-umbrella-name.md)). A mixer
  profile buys a process boundary and a clock bridge for something that must share the sample clock
  and the graph.

## Acceptance criteria

1. A versioned specification and seed produce a bounded, auditable musical realization with a
   deliberate ending. Arbitrary cells/parts are supported within declared resource limits.
2. At least two independently addressed CLAP instances make sound through the optional, separate
   host project; the platform core remains independent of CLAP and composition vocabulary.
3. The existing recorder captures the selected mix with known frame origin, correct channel count,
   and separately reported source overruns/bridge misses. No physical loopback is required.
4. Reopening a session reproduces its canonical event plan; reopening its recorded take needs no
   instrument. Audio reproducibility is measured per plugin rather than universally promised.
5. Stop, end, absent plugin, overflow, activation failure, and the chosen deployment's failure mode
   have observable outcomes; no hidden note drops or unbounded tail/route processing.
6. The selected host and recorder integration passes a ten-minute realtime soak; our audio-path
   allocation tests and event-capacity tests pass. Native third-party behaviour is documented honestly.
7. Existing workspace and shell tests remain green. Every implementation slice updates its owning
   note, and any in-process-host decision explicitly supersedes the earlier prohibition.

Before each slice lands, run the existing gates, plus its focused tests:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
node scripts/verify-agent-notes.mjs
```

The separate CLAP-host project and isolated shell workspaces have their own gates. A workspace test
run does not cover them. Add release-mode measurements for the selected renderer/bridge, and run
real-plugin tests explicitly rather than make ordinary CI depend on installed instruments.

## Risks

| Risk | Mitigation |
|---|---|
| Hosting becomes the whole project. | CLAP only; two candidates; one selected deployment; headless state/parameter support first; no general DAW feature list. |
| A correct generator still produces an uninteresting piece. | Reference listening after each musical slice; independent timescales, intentional relationships, and long-form development before more operators. |
| Float-chaos trajectories change across platforms or upgrades. | Version algorithms and numerical policy; persist the realized event plan; scope reproducibility claims. |
| Native plugin failure kills capture or GUI responsiveness. | Compare worker isolation; scan plugins outside the main process where practical; recover partial takes and report the affected interval. |
| A worker is mistaken for sample-accurate merely because it uses the same rate. | Frame tags, explicit pipeline latency, measured placement, shared clock authority, and a nonblocking deadline policy. |
| Dense experimental events overflow today's buffers. | Preflight actual per-block capacity; include on/off/control traffic; fail loudly and extend generic capacity only with tests. |
| Fast offline rendering overruns a realtime recording ring. | Keep first recording realtime-paced; a later offline path uses bounded control-side backpressure or streaming writes, never blocking the audio callback. |
| Long works exhaust in-memory export. | First release uses streaming capture. Existing export's roughly 1 GiB bound remains explicit; faster streaming offline export is a separate later slice. |
| Stems or GUI quietly enlarge the first release. | First capture is one mix take; first composer UI is headless. Confirm editor needs early because they affect host selection. |
| Recorder-first work is displaced without an explicit decision. | This is an optional new profile. Agree when its slices run; do not assume recorder alignment or other pending work is finished. |
| Cardinal's advertised CLAP support is mistaken for complete raw-CV support. | Qualify Synth/FX binaries; Main CV is not a supported CLAP assumption. Keep raw-CV and standalone bridges deferred. |
| Modular patch behaviour silently replaces the Rust composer. | Declare instrument versus process-instrument mode; save reset/state/asset contracts and preserve the platform's transport authority. |

## Open questions

1. Surge XT is the first real target; Cardinal Synth is proposed next. Should Cardinal initially be
   **only an instrument**, or should its patches also generate local musical material under Rust's
   macro/form control? Proposed default: instrument mode first.
2. Are native plugin editors required for the first release, or are saved states plus generic
   parameter controls enough? Which Linux display environment must editor support cover?
3. Should the first take capture the post-master stereo mix, the pre-master mix, or both eventually?
   Proposed initial default: one post-master stereo take, with its tap stated in provenance.
4. Does “experimental systems” prioritize rhythmic phase/polymeter, evolving sustained textures,
   or pitch/tuning systems? Proposed reference: independent rhythmic cycles plus one sustained part.
5. Is microtonality a first-release obligation or a later expressive-event extension? Do not reduce
   continuous pitch to MIDI integers silently if it is required.
6. Should existing browser systems be importable? Proposed default: a narrow importer for examples,
   with unsupported fields reported, not ongoing round-trip hardware compatibility.
7. What should the separate CLAP-host project be named, where should it live, and what licence should
   it use? Select these before repository creation, not during the library spike by accident.

## Deferred work

Endless/live-steered realization; composer checkpoints and seek acceleration; native editor
embedding; per-note expression and advanced tuning unless promoted above; simultaneous per-part
stems; live graph replacement; plugin-state morphing research; scripting/live coding; agent control;
ML generation; plugin-format export of our generators; VST3/LV2/AU; faster streaming offline bounce.
Raw-CV modular routing; standalone Cardinal OSC; Rack Free audio/sync bridge; Rack Pro qualification
until a concrete module/workflow need appears; autonomous modular generation until its ownership and
reset contract is agreed. Cardinal Synth/FX is a proposed follow-on, not part of the first finish line.

## Sources and verification

**Local verification:** Read the browser model and tests directly; ran `node test_fm1gen.mjs` with
all checks passing. Read sound-arranger at `8b2ebdf`, especially the following seams:

- [Clock and tempo](../../../../crates/engine/src/clock.rs),
  [graph/events](../../../../crates/engine/src/graph.rs),
  [plugin API](../../../../crates/engine/src/plugins/mod.rs), and
  [mount lifecycle](../../../../crates/engine/src/render.rs).
- [Recorder](../../../../crates/media/src/record.rs),
  [multi-channel capture](../../../../crates/media/src/capture.rs),
  [host recording API](../../../../crates/host/src/lib.rs), and
  [source declarations](../../../../crates/host/src/rig.rs).
- [Current capabilities](../../../../docs/capabilities.md) and the two prior architecture decisions
  linked above. Documentation may lag code; the source findings determine the integration gaps here.

**Provenance precondition:** the browser source is an **untracked working-tree snapshot** in the
studio repository (`?? studio/instruments/m-vave-fm-1/web/`), so nothing pins it. Commit it there
(or hash the fixtures) before Task 4 writes characterization fixtures — otherwise the comparison has
no fixed reference and none of the browser-model findings above can be re-checked.

**Web verification:** Used the harness's web search/fetch and direct HTTP reads of primary registry,
API, and upstream sources. No pi/model-recall round trip. Versions/licences/MSRVs come from registry
metadata; API availability and stub findings come from source/docs. No candidate was installed,
compiled, or proven against a real instrument during planning.

- [Clack host registry](https://crates.io/crates/clack-host/0.2.0),
  [versioned API](https://docs.rs/clack-host/0.2.0/clack_host/),
  [extensions registry](https://crates.io/crates/clack-extensions/0.2.0), and
  [upstream README/examples](https://github.com/prokopyl/clack).
- [Truce CLAP registry](https://crates.io/crates/truce-rack-clap/1.1.5),
  [versioned API](https://docs.rs/truce-rack-clap/1.1.5/truce_rack_clap/), and
  [upstream standalone limitations](https://github.com/truce-audio/truce-rack).
- [Raw CLAP bindings](https://crates.io/crates/clap-sys/0.5.0),
  [original Rack status](https://github.com/sinkingsugar/rack),
  [`plugin_host` CLAP source](https://docs.rs/plugin_host/0.1.0/src/plugin_host/bridge/clap.rs.html), and
  [its sandbox source](https://docs.rs/plugin_host/0.1.0/src/plugin_host/sandbox.rs.html).
- [CLAP event and transport specification](https://github.com/free-audio/clap/blob/main/include/clap/events.h).
- [Surge XT features and platform support](https://surge-synthesizer.github.io/),
  [its CLAP feature documentation](https://surge-synthesizer.github.io/clap), and
  [source repository](https://github.com/surge-synthesizer/surge).
- [Cardinal README/status](https://github.com/DISTRHO/Cardinal),
  [plugin variants](https://cardinal.kx.studio/variants),
  [host-parameter/module comparison](https://github.com/DISTRHO/Cardinal/blob/main/docs/DIFFERENCES.md),
  [standalone-only OSC](https://cardinal.kx.studio/osc-remote-control), and
  [current CLAP build targets](https://github.com/DISTRHO/Cardinal/blob/main/src/Makefile).
- [VCV Rack product boundaries](https://vcvrack.com/Rack) and
  [Rack Pro plugin/automation manual](https://vcvrack.com/manual/RackPro).

*Authored with GPT-6.1 Sol · OpenCode, 2026-10-08. The owner established the evolution, separate-host,
finite-first, and experimental-system direction and proposed Surge XT then Cardinal/VCV Rack;
library selection, deployment, and modular composition ownership remain proposed.*

*Amended with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-10-08 (light touch, owner's request): the
composer is a profile whose mount set folds in the existing `euclidean`/`scale` plugins and the CLAP
host as a plugin, with `tone` kept as the diagnostic voice; the detailed model is marked open and the
implementation plan a sketch; three alternatives added (existing generative program, MIDI to the
hardware, mixer-as-profile); factual corrections (per-node vs merged event capacity, "must not" for
`Plugin::apply`, no host graph tap today, provenance not a model); the untracked browser-source
precondition; and the section order brought to the notes format. No decision of the original author
is rewritten.*
