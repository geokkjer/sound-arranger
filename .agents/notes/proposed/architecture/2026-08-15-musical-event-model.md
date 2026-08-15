# Agent Note: Musical-event model — what is a musical event in a clip-based tool

Status: proposed

## Problem

The minimal-core note keeps the core model-free ("time, graph, events"), but the umbrella's first generators (euclidean rhythm, chord progression, bass) need *pitched events*, while the product's identity is clip-based audio with a locked "no MIDI note sequencing" decision (RESEARCH.md §3). "No MIDI" was decided for the *sequencing UI* (no piano roll, no note entry), not for the *platform vocabulary*. The core's event vocabulary must include a musical-event model, or every plugin invents its own and the log schema hardens around a guess. The model shapes the graph renderer (sample-playback nodes vs synth-voice nodes) and the log, so it must be decided before Phase 2.

## Proposal

Define the musical event as a *value*, not an instruction. Four event types in the log:

- **ClipEvent** — a Source region placed on the timeline: `{source_id, src_in, src_out, timeline_start, gain, pan, stretch, reverse, fades, warp_markers}`. The ACID-style unit: cut, paste, paint-to-fit.
- **NoteEvent** — a pitched event: `{pitch, start, duration, velocity, voice}` where **pitch is a real number in cents offset from a reference** (continuous pitch, not a tempered step). Matches the Feldman/texture identity (fine grain, long durations, non-goal-directed) while remaining quantizable to any grid *by plugins* — 12-TET is a quantization, not the representation.
- **ControlEvent** — parameter / automation changes, gesture-coalesced (start, curve segments, end — one fader ride, not thousands of `set_param` events).
- **MarkerEvent** — structural markers (section, cue, take).

**Time basis:** positions are stored in **absolute sample frames at a declared reference rate**; musical position (beats) is derived through the tempo map, so tempo-map edits never corrupt stored positions.

**Rendering:** NoteEvents render through synth-voice nodes (opaque node type); ClipEvents through sample-playback nodes (declarative). Both coexist in one graph value.

**The "no MIDI" lock stands** for the editing UI; generators produce NoteEvents as values, and the timeline simply offers no piano roll.

## Alternatives considered

- **MIDI-style note model (12-TET steps, velocity 0–127)** — familiar, but bakes a tempered grid into the substrate; the identity is texture/drone/fine grain (Feldman), so continuous pitch with quantize-as-plugin is the better base. Rejected for the substrate; a MIDI-compatible view can be a plugin later.
- **No note model at all (events only)** — the model-free extreme; but the graph renderer and log schema need a concrete vocabulary now, and generators need a shared target. Rejected: model-free means the substrate carries no *specific* model, not that it carries no vocabulary.
- **NoteEvents as ClipEvents of synthesized audio** — rendering pitches by materializing audio loses the generative/parametric quality (a pattern can no longer be edited live). Rejected for generators; still available per-voice via bounce.
- **Drop the "no MIDI" lock** — the lock is about the sequencing UI, not the vocabulary; no need to relitigate it.

## Acceptance criteria

- The log schema defines the four event types with the time-basis rule; a tempo-map edit does not corrupt stored positions.
- The euclidean plugin emits NoteEvents quantized to its grid through the clock's scheduler; the chord-progression plugin provides NoteEvents that a bass plugin consumes via `ctx.progression` — with no import between them.
- A synth-voice node and a sample-playback node coexist in one graph value.

## Risks

- **Continuous-pitch scope creep** — true microtonal rendering is hard. Mitigate: the representation allows it; rendering may stay near-12-TET initially, and microtonal precision is a later refinement.
- **Model fights the clip substrate** — no conflict: the Macero gestures (splicing, looping, dropping) remain edits over ClipEvents; the tape heritage stays inspiration, per RESEARCH.md §1.
