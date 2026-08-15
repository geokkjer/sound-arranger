# Agent Note: Minimal core — clock, graph interpreter, session log, context plumbing

Status: proposed

## Problem

The composition-seams note (2026-08-15-composition-seams-plugin-architecture.md) adopts the "everything is a plugin" discipline, but leaves the core undefined. sound-arranger-the-product is not the system: it is one capability (clip-based splicing and arranging — the ACID model) among many the user wants — euclidean rhythm generation, chord progressions, improv — under a higher-abstraction umbrella for music composition. Without a defined core, every capability would own its own clock, audio path, and history, which is how plugin systems die: no shared ground, plugins import each other, time drifts.

## Proposal

A minimal core of four pieces plus one core-privileged service; everything else is a plugin (see the composition-seams note).

1. **Clock / transport** — the single sample-accurate master clock: absolute time (sample frames at a declared reference rate) and musical time, quantization grid, swing. Core-owned because every plugin must agree on *when*; the rack spine (power / clock / bus), eurorack-style. No plugin owns time. It carries three non-negotiables: an **editable tempo/meter map** (beats ↔ samples — the Feldman seconds-timebase and the generators' beats both resolve against it), a **sample-accurate scheduling queue** ("emit this event at sample t", with lookahead into the audio thread), and a **clock-source seam** (internal / JACK transport / Ableton Link / MIDI clock later — the RESEARCH `TransportSync` seam, promoted into the core).
2. **Audio graph interpreter** — the realtime privileged place. The audio thread renders a graph *value* (nodes / edges / params) that plugins edit through reversible effects; the core reconciles and renders allocation-free (`rtrb` / `basedrop`). "The patch is a first-class value" (corpus `notes/modular-dsl-sketch.md`). Two node tiers: **declarative nodes** (serializable: clip playback, gain/pan, routing, sends — diffable, loggable, IPC-safe) and **opaque nodes** (a node type wrapping `Box<dyn AudioUnit>`: fundsp composites now, CLAP later — contributed via a Rust-side factory registry, running on the audio thread under RT discipline; its *existence and params* are values in the graph, its *code* is trusted in-process). Topology changes: build the new graph off-thread, swap an `Arc` atomically, retire the old via `basedrop`, crossfade at block boundaries; parameter changes via sample-timestamped SPSC queues with smoothing owned by the interpreter. **Audio teardown protocol**: reversal applies to the *contribution*, never the sound — unmounting ramps out / steals voices / flushes tails (you cannot un-ring a reverb). Fault model: `catch_unwind` off the audio thread, quarantine a poisoned plugin, FTZ/DAZ + denormal kill.
3. **Session event log** — append-only, core-owned. dsh's invariant applied to music: *model-visible means logged*. Anything a plugin needs to know (what was played, cut, changed) is an event; undo, fork, resume, replay, and improv all derive from it. A composition *is* a log — with the standard event-sourcing homework: events **reference audio by content hash** (never contain it); materialized snapshot + journal tail (snapshotting is itself a logged event); **gesture coalescing** (a fader ride is one gesture, not thousands of `set_param` events); replay determinism (*same log → byte-identical bounce*; pure folds, agent output logged not recomputed); **schema versioning from event #1**; positions stored in **absolute sample frames at a declared reference rate**, musical positions derived via the tempo map so tempo edits never corrupt stored positions.
4. **Context plumbing** — services (`ctx.clock`, `ctx.audio`, `ctx.session`, `ctx.parts`, `ctx.progression`, …), `inject` dependencies, typed events, reversible effects (Cordis, or the pattern; TypeScript host side, behind the value boundary).
5. **Media engine — core-privileged, not a plugin** — disk streaming for long-form audio: per-source reader threads, read-ahead ring buffers, underrun policy, the peak-pyramid cache; the recording writer (ring-fed thread, WAV header finalization, crash recovery of in-progress takes); input↔output device-clock drift reconciliation. Every profile needs it and it must meet the audio deadline — a recorder *plugin* owns policy (when to record, silence auto-split), never the safe-write machinery.

**Not in the core:** tracks, parts, notation, note models — the visual clip model and the tape metaphor are organizational models (plugins / profiles), not the substrate. The composition is the log + graph; views edit it. The tape heritage (Macero's splicing, looping, drop-the-track) is *inspiration*: the modern visual computer makes those techniques easier — clips as objects, cut/paste, paint-to-fit time-stretch (the ACID model).

**The umbrella:** a named composition of plugins — a profile. "sound-arranger" becomes the **clip-arranger profile** (recorder + cut/paste clip editor + soft mixer), ACID-style: clips as objects, cut and paste, paint-to-fit stretch — the modern computer paradigm, with the tape techniques as inspiration. The product is the assembled system.

**First plugins:** euclidean rhythm (pure generator `euclid :: Steps -> Pulses -> Rot -> Pattern`, injects `ctx.clock`), chord progression (provides `ctx.progression`; the bass plugin injects it), improv (consumes the log and the progression/rhythm services, writes a part back — the paper's self-evolving component; made *auditable* by the log, not reversible — see the musical-event model note).

**Phase-0 spikes redefined:** *Spike A* — core (clock with tempo/meter map, graph interpreter, session log, context) + one euclidean plugin driving a sine blip, plus a log-replay check (replay ⇒ identical output); proves the paradigm. *Spike B* — record 20–30 min while playing a long file from disk, splice a clip *during playback* glitch-free, bounce; proves the product's hard core (streaming, writer, drift, edit-while-playing, graph swap under load). Both constrain the core shape before it hardens.

## Alternatives considered

- **sound-arranger-the-product as the core** — the original framing; the tape model (tracks/clips) leaks into the substrate and every future capability must import it. Rejected: the core must be model-free (time, graph, events only).
- **Clock as a plugin** — a plugin could own time, but cross-plugin sync then depends on one plugin being loaded, breaking the "plugins never coordinate order" guarantee. Rejected: the clock is the context made temporal, not a service.
- **Session log as a plugin** — same reasoning: improv agents cannot depend on "whatever happened to be loaded" for the shared record. Rejected: the log is core-owned, like dsh's session log.
- **Adopt an existing host as the core (Tidal / SuperCollider / VCV Rack)** — mature (the corpus documents Tidal; tidal-lsp exists), but the goal is a model-free minimal substrate in our own stack (Rust core + TS host); existing hosts become plugin providers later (e.g. a Tidal provider). Deferred, not excluded.
- **Full DAW core (timeline + audio + plugins as one)** — the traditional product shape; rejects the higher-abstraction umbrella. Rejected per the composition-seams note.
- **The tape metaphor as the substrate** — the original framing; the model leaks tape's constraints (reels, splicing blocks, no undo). Rejected: the tape techniques are inspiration; the substrate is the visual clip model, where every tape gesture is easier and reversible.

## Acceptance criteria

- Phase-0 spikes A and B pass: clock with tempo map; euclidean plugin drives a sine blip in time; log replay produces identical output; a 20–30 min record + edit-during-playback + bounce completes glitch-free.
- Unmounting a plugin leaves no residual sound *or state* — via the audio teardown protocol (ramp / voice-steal / tail-flush), not by pretending the sound was inverted.
- Two plugins that never import each other cooperate via the log and a service key (e.g. euclidean provides rhythm; a bass-note provider injects it) — proving spatial composability.
- Same log → byte-identical bounce.
- The core plugin contract (ctx keys, core event types, graph value types, the two-tier node model) is written down — this replaces the unwritten "IPC command list".

## Risks

- **Core creep** — tracks/clips sneak back in as "just one more core feature". Mitigate: the core grows only by the four pieces; everything else must be a plugin or a profile, enforced by the acceptance criteria.
- **Realtime interpreter complexity** — the graph renderer must stay allocation-free as graphs grow. Mitigate: value-based graph diffing, tested under load; plugin code kept off the audio thread by construction.
- **Over-abstraction** — the umbrella may never need more than the tape editor. Mitigate: profiles keep it usable as a plain arranger; the plugin discipline costs nothing while unused.
- **Split-brain composition** — context plumbing on the TypeScript side, composed objects (nodes, recorder, streaming) in Rust. Mitigate: the graph value is the contract — a node's contribution is a value; the Rust runtime owns node lifecycle and reversibility; the TS loader composes engine-level config. One authority, stated not implied.
- **Spike A validates the paradigm, not the product** — mitigated by Spike B; if streaming and the writer force changes to the core shape, that is the cheapest possible moment to learn it.
