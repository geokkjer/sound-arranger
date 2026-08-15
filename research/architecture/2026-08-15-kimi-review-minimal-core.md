# Second opinion — kimi-cli review of the minimal-core architecture

> **Source:** `kimi-cli` non-interactive review, 2026-08-15 (kimi session `session_db378242-7ed9-4cf2-bf01-b75abc6040a5`).
> **Reviewed:** `RESEARCH.md` (§11, §15) and the composition-seams + minimal-core notes.
> **Disposition:** folded into the minimal-core note (media engine as core-privileged, tempo/meter map + sample-accurate scheduler + clock-source seam, two-tier node model, audio teardown protocol, log infrastructure rules, Spike B); prompted the new musical-event-model note; the tape framing was demoted to *inspiration* in `RESEARCH.md` §1 per user decision. Full text preserved below verbatim.

---

## 1. Gaps and blind spots — what generic plugin-architecture advice misses about realtime audio

The Cordis/dsh paradigm was proven on chatbots and agent harnesses: asynchronous, event-driven, no hard deadlines, and — critically — an environment whose state is *reversible data*. Realtime audio violates several of its quiet assumptions:

**Reversal is physically impossible on the audio path.** The paper's temporal composability assumes every effect Γ → Γ has an inverse. Audio state has none: you cannot un-ring a reverb tail, un-play a sample through a delay line, or retract a click that already hit the DAC. "Unmount the euclidean plugin and hear silence" only works if the core defines an *audio teardown protocol* — ramp-out / voice-steal / tail-flush — which is a domain mechanism, not a derived inverse. The note's acceptance criterion "unmounting removes its contribution (no residual sound or state)" is achievable, but only by building that protocol explicitly. Right now the notes assume the paper gives it to you for free. It doesn't.

**Rate separation is unaddressed.** A music system has at least three rates: audio-rate (per-sample), control-rate (per-block or per-event, sample-timestamped), and application-rate (UI, edits, agent decisions). Cordis events are all application-rate. "Model-visible means logged" must be scoped to application-rate facts only — the moment someone logs control-rate parameter streams naively (dub fader rides at 30–100 Hz over a 45-minute jam), the log explodes. The core needs an explicit statement of which rates exist and which are loggable.

**Disk streaming is the elephant.** The product is "record long live jams." An hour of 48kHz/24-bit stereo is ~1 GB; a multitrack session is several. You cannot hold sources in RAM — playback of long clips requires a disk-streaming engine with read-ahead ring buffers, per-file reader threads, and underrun policy. This is realtime-critical infrastructure with the same thread-safety discipline as the graph interpreter, and it appears in *neither* note. It looks like it belongs to the "recorder plugin," but a plugin cannot own it: every clip-playing profile needs it, it must satisfy the audio thread's deadline, and it interacts with the clock (buffer refill is scheduled against playback position). This is either a fifth core piece or a core-adjacent privileged service. Its absence is the single biggest technical blind spot — and the Phase-0 sine-blip spike will never touch it (see §8).

**Two hardware clocks.** "The single sample-accurate clock" papers over the fact that input and output devices drift against each other (separate crystals, ppm-scale). Recording while playing (overdub, or even just monitoring) requires drift estimation and resampling or sample-drop compensation. cpal won't do this for you. One software master clock is correct; the core must also own the *device-clock reconciliation* story.

**Plugin delay compensation.** The moment an effect with lookahead (limiter, linear-phase EQ, resampling stretch) enters the graph, tracks go out of sync unless the graph value carries per-node latency and the interpreter compensates. If "the patch is a value," latency must be part of that value's schema from the start — retrofitting PDC into a graph format is painful.

**Recording I/O mechanics.** `hound` writes need their own thread fed by a ring buffer, WAV header patching on stop, and crash-recovery of in-progress takes (a 40-minute jam lost to a panic is the worst failure this product can have). Nobody owns this in the four-piece core.

## 2. Risks — what makes this fail in practice

**The spike validates the paradigm, not the product.** Phase 0 (euclid → sine blip) exercises the clock, the graph swap, and mount/unmount. It does not exercise disk streaming, recording, edit-during-playback, peak generation, or the waveform pipeline — the things the actual product lives or dies on. You risk shipping a beautiful plugin kernel whose core shape is wrong for tape editing, discovered in Phase 1 when it's expensive.

**Building a framework with zero plugins.** Koishi and dsh sustain "everything is a plugin" because they have hundreds of plugins and thousands of users demanding extensibility. You have one profile and a hobby user base of one. The failure mode is the classic one: the composition machinery (loader, patches, reversible effects, ctx keys, layered config) becomes the project, and the arranger — the thing you'll actually use to make music — slips. The notes acknowledge over-engineering as a risk but the mitigation ("start with one plugin domain") is weaker than the pull. I'd state the budget honestly: the plugin discipline earns its keep only if the recorder seam and the OfflineProcess seam are *exercised by real second providers* within Phase 1–2, otherwise it's ceremony.

**Two plugin systems across a language boundary.** The notes put context plumbing (inject, reversible effects, ctx keys) on the TypeScript side and trait seams on the Rust side, joined by "the value boundary" / IPC. That is *two* composition runtimes, and every nontrivial plugin must decide which side it lives on — or split across both. A fundsp effect plugin is Rust-side (it's DSP); its panel and automation are TS-side; its graph contribution crosses the boundary as a value. Who owns its lifecycle, its reversibility, its inject declarations? The notes say "composition lives on the Tauri side," but the effects being composed (registering a graph node, opening a device) happen in Rust. This split-brain is the most likely source of architectural incoherence in practice. It needs one sentence of resolution: the Rust engine is itself a plugin *runtime* with trait-seams and its own reversible registration, and the TS loader composes engine-level config — or the reverse. Pick one authority.

**Graph-as-value vs fundsp (see §5)** — a concrete technical risk hiding inside the interpreter promise.

## 3. The session log as core — right choice, with standard event-sourcing homework

Event sourcing for the edit model is *right* — it's the strongest part of the proposal. Undo, fork, replay, and an improv agent reading "what happened" all follow from it, and "model-visible means logged" is a good invariant to steal. But the pitfalls are well-known and the notes mention none of them:

- **Events reference audio; they never contain it.** Clips reference Sources by content hash/path; peaks and audio live outside the log. This needs saying, because "a composition IS a log" invites the misreading that the log is self-contained.
- **Compaction and snapshots are not optional.** A dub performance recorded as automation, plus markers, edits, and agent output over months of sessions = a log that must not be re-folded on every launch. Standard solution: materialized snapshot + journal tail, with snapshotting as a logged event. Define it now or Phase 1 performance work rediscovers it the hard way.
- **Gesture coalescing.** A fader ride is one *gesture* (start, curve segments, end), not 2,700 `set_param` events. Log gestures; derive the stream. This also makes undo sane (undo the ride, not one sample of it).
- **Replay determinism.** If replay drives the improv agent and any bounce must be reproducible, the fold functions must be pure: no wall clock, no unseeded randomness, agent output logged as events rather than recomputed. Worth one acceptance criterion: *same log → byte-identical bounce.* Cheap to test, proves the whole claim.
- **Schema versioning from event #1.** The log is durable; you'll be reading v1 events in a year. Version the envelope now.
- **Time basis of events.** This interacts with §4: an event's position must be stored in a basis that survives tempo-map edits — typically absolute sample position at a declared reference rate, with musical position derived via the map (or both, with one authoritative). Get this wrong and the first tempo change corrupts every edit.
- **Memory** is then a non-issue: with audio excluded and gestures coalesced, the log is megabytes per year.

So: yes, keep it in core — but "append-only log" is the beginning of the design, not the end. Snapshot, compaction, coalescing, versioning, and time basis are the actual decision content.

## 4. One clock is right — but "a clock" is not enough clock

A single sample-accurate master clock as core is correct — the audio device clock is the only honest time base, and "no plugin owns time" is the right rule. But a bare "now" is insufficient for music:

- **The tempo/meter map must ship with the clock core.** Your own plan contradicts a seconds-only world: the euclidean and chord-progression plugins are Phase 2 *and the Phase-0 spike itself*, and they speak beats. Meanwhile RESEARCH.md locks a seconds-based timebase for the tape profile. Both are fine — the resolution is a clock core that owns the sample counter *plus* an editable tempo/meter map (beats ↔ samples) — the map mutated via logged events like everything else. Without it, every generator plugin invents its own beat math and you're back to "time drifts," the exact failure the note invokes to justify core ownership.
- **Scheduling, not just reading.** Generators need "emit this event at sample t" with lookahead into the audio thread (the standard pattern: the control side schedules events a few hundred ms ahead, timestamped; the renderer pops them at the exact frame). The clock core must provide the scheduling queue and the query API ("which pulses fall in this block?"), or each generator rolls its own and sample accuracy is lost at the first buffer-boundary bug.
- **External clock sources are a when, not an if.** JACK transport is explicitly in your latency doc's world; Ableton Link and MIDI clock are the obvious "jam with the Pi rig" features. The core clock should define a *source* abstraction (internal / JACK / Link) behind the one master — the RESEARCH already lists a `TransportSync` seam; promote that from "later" to "the clock core's provider seam" so the abstraction exists before the first external sync lands.
- **Varispeed interacts with the clock.** Tape varispeed changes the rate at which timeline position advances through source material. If musical time must track a varispeeded deck, the map needs rate segments. Decide whether varispeed is purely a graph-node parameter (timeline time unaffected — simplest, probably right) or a clock effect, and write it down.

So: one clock, yes — but the core piece is "clock + tempo map + sample-accurate scheduler + clock-source seam," not "a clock."

## 5. Value-based graph diffing — workable, and you have a real design tension to resolve

The approach is sound and has excellent precedent — but the notes currently promise something fundsp can't deliver, and don't name the actual mechanism.

**What real engines do:**

- **SuperCollider** is your closest ancestor and the strongest evidence this works: scsynth renders a graph of *fixed UGens*; the language side edits the graph by sending value-level messages (node IDs, params); new synth definitions are compiled offline and loaded atomically. The server never runs user code. The price: the node vocabulary is closed — third-party DSP arrives as compiled UGens, i.e., *server extensions*, not plugins in the Cordis sense.
- **JUCE / the industry norm** does the opposite: plugins absolutely run on the audio thread (`processBlock`), and the host's job is marshalling parameter changes through lock-free queues and swapping topology off-thread with RCU-style handoff. The boundary is "plugin code must obey RT rules," enforced by convention, not "no plugin code on the audio thread." VST3/CLAP both standardize exactly this: `process()` on the audio thread, parameter events as timestamped value queues.
- **fundsp** is "graph as value" only in the FP sense — it's composed Rust closures (`Box<dyn AudioUnit>`), not serializable data, and rebuilding a graph allocates; it is not allocation-free across topology changes. You can hold fundsp graphs behind an atomic swap, but you cannot *diff* them or ship them over IPC as data.

**The tension the notes don't resolve:** "plugins edit a graph *value* (nodes/edges/params)" requires a closed node registry — the interpreter can only render node types it knows. That works for the euclid spike and for a fixed mixer, but your own Phase 2 plan says fundsp effects *are* plugins, and Phase 4 says CLAP hosting. Both are opaque code that must execute on the audio thread. So the real design is a **two-tier node model**, and it should be written into the note:

1. **Declarative nodes** — the serializable value layer: clip playback, gain/pan, routing, sends, the fundsp building blocks you choose to expose. Diffable, loggable, reversible, IPC-safe.
2. **Opaque nodes** — a node type that wraps `Box<dyn AudioUnit>` (fundsp composites now, CLAP later), contributed by Rust-side plugins through a factory registry, running on the audio thread under RT discipline. Its *existence and params* are values in the graph (hence logged and reversible); its *code* is trusted in-process code, exactly like a SC UGen or a CLAP plugin.

Once that's stated, the mechanics are well-trodden and I'd prescribe them rather than rediscover them: **don't diff at fine granularity initially** — build the new graph off the audio thread, swap an `Arc` atomically, retire the old one via `basedrop`, and take topology changes at block boundaries with a short equal-power crossfade to mask discontinuities. Parameter changes go through SPSC queues as sample-timestamped events with per-parameter smoothing (linear/exp ramps) *in the interpreter* — zipper noise handled once, in core, not re-implemented per plugin. Fine-grained diffing is a later optimization; "value-based graph diffing" as currently phrased oversells what you need on day one and undersells the swap machinery you actually need.

## 6. Missing core pieces

Things that look like plugins but are core or core-privileged:

- **Disk streaming / media engine** — §1; the biggest one. Reader threads, read-ahead, underrun policy, per-source peak pyramids (the peak cache is also shared infrastructure, not per-profile).
- **Device I/O ownership and drift reconciliation** — the cpal callback(s), input↔output clock alignment, sample-rate conversion of sources against device rate.
- **The recording pipeline** — writer thread, header finalization, crash recovery. A recorder *plugin* can own policy (when to record, auto-split on silence); the safe-write machinery must be core-adjacent.
- **Parameter/automation model** — smoothing primitives and automation lanes. Every profile needs them; if they're per-plugin, you get four zipper-noise implementations and inconsistent feel.
- **Voice management** — the moment euclid drives anything polyphonic you need voice allocation/stealing. Currently invisible in the core because the spike is one blip.
- **Log infrastructure** — snapshot/compaction/versioning per §3.
- **Fault model** — what happens when a plugin panics or a graph node NaNs. `catch_unwind` off the audio thread, poisoned-plugin quarantine, denormal handling (FTZ/DAZ on the audio thread, denormal-kill in the graph). Cordis gives you lifecycle discipline for free on the TS side; the Rust core needs its own answer, stated once.

## 7. Incoherences in the umbrella vision

- **"No MIDI note sequencing" vs. generators.** §3 locks "no MIDI, no note sequencing" as product identity; the umbrella's first plugins are a chord progression provider and a bass plugin that injects it — i.e., pitched note events. This isn't fatal (the product's UI has no piano roll; the platform has note *events*), but the core's event vocabulary must then include a musical-event model — pitch, time, duration, articulation — and nobody has defined it. "The core is model-free (time, graph, events)" quietly smuggles in the hardest model question of all: what *is* a musical event in a tool whose identity is tape? Continuous pitch? 12-TET notes? Both? This needs a note of its own before Phase 2, because it shapes the graph renderer (sample playback vs. synth voices are different node families) and the log schema.
- **"Made safe by core reversibility" overclaims.** Effect reversal makes an improv agent's *registrations* clean; it does nothing for the musical safety of its *output*. What makes the agent safe is that its output lands in the log as ordinary, undoable, forkable events — auditability, not reversibility. Say that instead; the current phrasing borrows authority the mechanism doesn't have.
- **Phase ordering contradicts risk ordering.** The umbrella (euclid, improv — the paper's "self-evolving endgame") is validated in Phase 0; the product (the tape arranger, which is the stated goal of the entire research doc) waits for Phase 1 with a core never stress-tested against its problems. If the umbrella is genuinely the goal, fine — but the documents keep calling the arranger "Phase 1 (the goal)." One of these is the real priority; the architecture review can't settle that, but the current ordering optimizes for the framework story.
- **TS-side composition vs Rust-side reality** — §2's split-brain point, restated as an umbrella issue: the "assembled profile" is described as a TS-side config tree, but most of what a profile assembles (recorder, graph nodes, streaming, mixer) is Rust. The profile concept has to span both, or it's a UI skin over an uncomposed engine.

## 8. What I'd do differently in the first spike

The euclid-blip spike is a good *plugin-paradigm* spike and a bad *product* spike. Since the core's shape will be set by what you validate now, run **two spikes in parallel or quick succession**, and let both constrain the core:

**Spike A (as planned, plus):** clock + graph + log + ctx, euclid → sine blip, live mount/unmount. Add three things: (a) the tempo/meter map, even trivially (euclid needs beats anyway — so the spike *already* forces the clock question of §4; better to face it); (b) a log-replay test — replay the session and assert identical output, proving the event-sourcing claim for €0; (c) one opaque fundsp node alongside declarative gain, proving the two-tier node model of §5 before the graph value schema hardens.

**Spike B (the one that's missing):** record 20–30 minutes of audio while playing back a long file from disk, razor-splice a clip *during playback* without a glitch, move it, bounce to WAV. This exercises disk streaming, the recording writer, drift, edit-during-playback, and the graph swap under real load — the product's actual hard core. If the four-piece core survives Spike B unchanged, you have evidence the architecture fits the product. If it doesn't (my bet: streaming and the recording path force their way into the core), you learned it at the cheapest possible moment.

Also: write the two-tier node model and the log's snapshot/coalescing/versioning rules into the minimal-core note *before* the spike, as hypotheses the spike must falsify. Right now the acceptance criteria test that the paradigm works; they don't test anything that could fail.

---

**Bottom line.** The four-piece core is a defensible, even elegant, decomposition, and the privileged-realtime-kernel honesty is exactly right. But the proposal as written is a plugin-architecture document that happens to be about audio, not yet an audio architecture with a plugin story. The gaps are all in the audio-specific substrate: disk streaming, device-clock drift, the recording pipeline, tempo map + scheduling, latency compensation, parameter smoothing, and the declarative-vs-opaque node tension in the graph. Resolve those six, run Spike B, and demote "reversibility" from load-bearing metaphor to what it actually buys you (clean lifecycles on the control side). The rest of the paradigm — seams, log, ctx — survives scrutiny.
