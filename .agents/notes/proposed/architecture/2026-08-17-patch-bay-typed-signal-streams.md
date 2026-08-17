# Agent Note: Patch bay — typed signal streams between plugins, and the external I/O seams (MIDI, OSC, SC, Tidal)

Status: proposed

## Problem

The product goal is hardware-style modularity from the start: a pure pattern generator (euclidean) must be patchable into a pitch/scale generator and then a tone generator, with inputs ranging from MIDI to our own generators (user requirement, 2026-08-17). Spike A hardwires euclidean's triggers to the blip voice it creates — right for the spike, wrong for the product: without named, typed ports, "plugin outputs" do not exist and nothing can be plugged into anything. The theory corpus already specifies the model (`notes/modular-dsl-sketch.md`: `Rate = Audio | Control | Trigger | Gate`, `Port(dir, rate)`, the patch as a first-class value). External I/O (MIDI, OSC, SuperCollider, Tidal) needs seams, or every integration becomes a special case. The graph schema freezes at Phase 1, so the port model must land before it (Spike A.5).

## Proposal

1. **The graph value becomes a patch bay.** Nodes declare named, typed ports; patch cords (`from_node.port → to_node.port`) replace the single-input chain. Signal kinds: **audio** (sample buffers), **control** (block-rate), **trigger** / **note** (sample-timestamped event streams — the Spike A scheduler mechanism generalizes into an event router). Connections are type-checked at connect (audio→audio, note→note, trigger→trigger); a mismatch is refused, fail-loud like `inject`. Example chain: `euclidean.out("triggers") → scale.in("trigger") → scale.out("note") → tone.in("note") → tone.out("audio")`.
2. **Streams vs log.** Live patch streams (per-block, transient) are *derived* from logged events: mounting a plugin and patching it is logged (determinism holds: same log ⇒ same signal); the streams flow at render time. The [musical-event model note](2026-08-15-musical-event-model.md)'s `NoteEvent` stays the durable log fact; a Note *stream* is its live form.
3. **Scale/pitch as pure functions + a service.** `scale :: Scale -> Note -> Freq` as pure combinators; a `ctx.scale` provider for named scales. Pure, testable, and the natural target of the patch UI.
4. **External I/O seams — traits defined now, implementations when demanded (budget rule):**
   - `MidiSource` / `MidiSink` — note on/off/CC in → Note/Control events; our events → MIDI out; MIDI-learn (RESEARCH §8). MIDI is an *external protocol on this seam*, never the internal note vocabulary (continuous pitch, musical-event note).
   - `OscSource` / `OscSink` — receive sample-timestamped OSC bundles → typed events; our events → OSC. The Tidal/SC interop layer: SuperCollider (scsynth) is an `OscSink` provider (notes → `/s_new`); a SuperDirt-compatible endpoint (receive `/dirt/*`) makes Tidal able to drive our engine. Both are ordinary providers on the seam.
   - JACK as the inter-process *audio* transport (pi rig precedent), gated on the media engine (Spike B).
5. **The patch UI is generated from ports.** The "dropdown of available inputs" is a rendering of the port declarations: for each input port, list providers of the matching signal kind (generators, MIDI, OSC, services) — the same principle as dsh's tool catalog: the surface is generated from the seams.

## Alternatives considered

- **Keep hardwired plugin→voice coupling (status quo)** — every new pairing (euclidean→pitch→tone, MIDI→tone, OSC→anything) becomes a bespoke code path. Rejected: patching is the stated product goal.
- **OSC/MIDI only at the app boundary (no core change)** — external I/O without internal patchability leaves the internal problem unsolved (euclidean still cannot feed a pitch generator). Rejected: both are the same mechanism — typed event streams into and out of the patch bay.
- **Full VCV-style CV model (per-sample control voltages everywhere)** — powerful but overkill; the typed-kind model (audio/control/trigger/note) covers the need and matches the corpus sketch. Deferred.
- **MIDI as the internal event format** — bakes a tempered grid into the substrate (the [musical-event model note](2026-08-15-musical-event-model.md) already chose continuous pitch). Rejected: MIDI is an external protocol, not the internal vocabulary.

## Acceptance criteria

- A test patches `euclidean.triggers → scale.trigger → tone.note` and renders audio whose pitch follows the scale (not a fixed freq).
- A type-mismatch patch (audio→trigger) is refused at connect.
- Patching is logged; replay reproduces the identical signal (determinism holds under the port model).
- `MidiSource` / `OscSource` trait definitions exist; a test injects a fake OSC source as a trigger provider, and the patch UI model lists it among providers of the matching kind.

## Risks

- **Port-model churn before Phase 1** — the schema freezes at Phase 1; landing ports in Spike A.5 is the cheapest moment. Mitigate: keep the port model minimal (name, direction, kind); fan-out/fan-in mixing stays with the Phase 1 mixer.
- **Voice allocation reappears** — patching implies polyphony; keep voice management deferred (RESEARCH §14 risk 8) but design the Note stream to carry a voice hint.
- **OSC/SC integration scope creep** — SC-as-plugin and Tidal-as-backend are Phase 2+; the seam *definitions* cost nothing now, implementations wait for demand.
