# Agent Note: Minimal core — clock, graph interpreter, session log, context plumbing

Status: proposed

## Problem

The composition-seams note (2026-08-15-composition-seams-plugin-architecture.md) adopts the "everything is a plugin" discipline, but leaves the core undefined. sound-arranger-the-product is not the system: it is one capability (tape-style splicing and arranging) among many the user wants — euclidean rhythm generation, chord progressions, improv — under a higher-abstraction umbrella for music composition. Without a defined core, every capability would own its own clock, audio path, and history, which is how plugin systems die: no shared ground, plugins import each other, time drifts.

## Proposal

A minimal core of four pieces; everything else is a plugin (see the composition-seams note).

1. **Clock / transport** — the single sample-accurate clock: musical and absolute time, quantization grid, swing. Core-owned because every plugin must agree on *when*; the rack spine (power / clock / bus), eurorack-style. No plugin owns time.
2. **Audio graph interpreter** — the realtime privileged place. The audio thread never runs plugin code: it renders a graph *value* (nodes / edges / params) that plugins edit through reversible effects; the core reconciles and renders, allocation-free (`rtrb` / `basedrop`). "The patch is a first-class value" (corpus `notes/modular-dsl-sketch.md`).
3. **Session event log** — append-only, core-owned. dsh's invariant applied to music: *model-visible means logged*. Anything a plugin needs to know (what was played, cut, changed) is an event; undo, fork, resume, replay, and improv all derive from it. A composition *is* a log.
4. **Context plumbing** — services (`ctx.clock`, `ctx.audio`, `ctx.session`, `ctx.parts`, `ctx.progression`, …), `inject` dependencies, typed events, reversible effects (Cordis, or the pattern; TypeScript host side, behind the value boundary).

**Not in the core:** tracks, clips, parts, notation — the tape metaphor is one organizational model (a plugin / profile), not the substrate. The composition is the log + graph; views edit it.

**The umbrella:** a named composition of plugins — a profile. "sound-arranger" becomes the tape-editor profile (recorder + splice/loop arrange + soft mixer). The product is the assembled system.

**First plugins:** euclidean rhythm (pure generator `euclid :: Steps -> Pulses -> Rot -> Pattern`, injects `ctx.clock`), chord progression (provides `ctx.progression`; the bass plugin injects it), improv (consumes the log and the progression/rhythm services, writes a part back — the paper's self-evolving component, made safe by core reversibility).

**Phase-0 spike redefined:** core (clock + graph + log + ctx) + one euclidean plugin driving a sine blip — proves sample-accurate clock, realtime-safe graph rendering, and live mount/unmount in a single spike.

## Alternatives considered

- **sound-arranger-the-product as the core** — the original framing; the tape model (tracks/clips) leaks into the substrate and every future capability must import it. Rejected: the core must be model-free (time, graph, events only).
- **Clock as a plugin** — a plugin could own time, but cross-plugin sync then depends on one plugin being loaded, breaking the "plugins never coordinate order" guarantee. Rejected: the clock is the context made temporal, not a service.
- **Session log as a plugin** — same reasoning: improv agents cannot depend on "whatever happened to be loaded" for the shared record. Rejected: the log is core-owned, like dsh's session log.
- **Adopt an existing host as the core (Tidal / SuperCollider / VCV Rack)** — mature (the corpus documents Tidal; tidal-lsp exists), but the goal is a model-free minimal substrate in our own stack (Rust core + TS host); existing hosts become plugin providers later (e.g. a Tidal provider). Deferred, not excluded.
- **Full DAW core (timeline + audio + plugins as one)** — the traditional product shape; rejects the higher-abstraction umbrella. Rejected per the composition-seams note.

## Acceptance criteria

- Phase-0 spike: the core runs; a euclidean plugin mounted from config drives a sine blip in time; unmounting it removes its contribution (no residual sound or state).
- Two plugins that never import each other cooperate via the log and a service key (e.g. euclidean provides rhythm; a bass-note provider injects it) — proving spatial composability.
- A plugin's generated output is fully reverted on unload — temporal composability.
- The core plugin contract (ctx keys, core event types, graph value types) is written down — this replaces the unwritten "IPC command list".

## Risks

- **Core creep** — tracks/clips sneak back in as "just one more core feature". Mitigate: the core grows only by the four pieces; everything else must be a plugin or a profile, enforced by the acceptance criteria.
- **Realtime interpreter complexity** — the graph renderer must stay allocation-free as graphs grow. Mitigate: value-based graph diffing, tested under load; plugin code kept off the audio thread by construction.
- **Over-abstraction** — the umbrella may never need more than the tape editor. Mitigate: profiles keep it usable as a plain arranger; the plugin discipline costs nothing while unused.
