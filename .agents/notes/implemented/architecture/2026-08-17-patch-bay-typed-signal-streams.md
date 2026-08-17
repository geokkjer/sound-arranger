# Agent Note: Patch bay — typed signal streams between plugins, and the external I/O seams (MIDI, OSC, SC, Tidal)

Status: implemented

## Problem

The product goal is hardware-style modularity from the start: a pure pattern generator (euclidean) must be patchable into a pitch/scale generator and then a tone generator, with inputs ranging from MIDI to our own generators (user requirement, 2026-08-17). Spike A hardwired euclidean's triggers to the blip voice it created — right for the spike, wrong for the product: without named, typed ports, "plugin outputs" do not exist and nothing can be plugged into anything. The theory corpus already specified the model (`notes/modular-dsl-sketch.md`: `Rate = Audio | Control | Trigger | Gate`, `Port(dir, rate)`, the patch as a first-class value). External I/O (MIDI, OSC, SuperCollider, Tidal) needs seams, or every integration becomes a special case. The graph schema freezes at Phase 1, so the port model had to land before it.

## Decision

Spike A.5 ships the patch bay in `crates/engine`:

- **Nodes declare named, typed ports** (`Port { name, direction, kind }`; kinds: audio / control / trigger / note; at most one port per kind per direction in Spike A.5). **Patch cords** replace the single-input chain. Connections are type-checked at `engine.patch` (kinds + directions, against the declared `port_table`) and again at `graph.connect` (kinds, directions, forward order, control single-driver). Refusals are fail-loud at patch time and a refused mutation is never logged.
- **Signal kinds**: audio = sample buffers; control = one f32 per block (single-driver in Spike A.5); trigger / note = sample-timestamped event streams in fixed-capacity buffers (`EventBuf`, 32 events/block out, 128 merged). **Event fan-in keeps the merged stream sorted by offset** (`insert_sorted`) — consumers like the tone voice drain by offset, so an unsorted merge would stall or corrupt them.
- **Generators are self-contained nodes** — the separate `Generator` machinery is gone. `euclidean` mounts `EuclideanGen` with `out("triggers")` and provides the `rhythm` service; `scale` mounts `ScaleGen` (`in("trigger")` → `out("note")`, a pure per-trigger degree counter); `tone` mounts `ToneGen` (`in("note")` → `out("audio")`, per-note pitch, the Spike A.5 master out). The canonical chain `euclidean.triggers → scale.trigger → scale.note → tone.note → tone.audio` is data, not code.
- **PDC delay lines are preallocated** to `MAX_PDC` (64 samples) at node creation; the render path only changes read offsets — no allocation, no rebuild. Node latency is fixed at mount in Spike A.5.
- **Patching is logged** (`Event::Patch` carries `at_frame`) and applied by the render loop at its frame; replay reproduces byte-identical output. `apply_patch` never panics on the audio path: un-mounted endpoints are a debug-asserted log-order error (skipped in release), and apply-time refusals (forward order, single-driver) are logged-as-intent and skipped — replay reproduces the same refused state.
- **The external I/O seams exist as traits**: `EventSource` / `EventSink`, `MidiSource` / `MidiSink`, `OscSource` / `OscSink` (SuperCollider = an `OscSink` provider, notes → `/s_new`; Tidal = a SuperDirt-compatible `OscSource` endpoint later). Implementations arrive when Phase 2 integrations demand them (budget rule).
- **The dropdown's data source**: `providers_of` / `provider_names_of` list every mounted `Out` port of a kind — by node id and by plugin name. The patch UI is a rendering of the port declarations.

## Alternatives considered

- **Keep hardwired plugin→voice coupling (status quo)** — every new pairing (euclidean→pitch→tone, MIDI→tone, OSC→anything) becomes a bespoke code path. Rejected: patching is the stated product goal.
- **OSC/MIDI only at the app boundary (no core change)** — external I/O without internal patchability leaves the internal problem unsolved. Rejected: both are the same mechanism — typed event streams into and out of the patch bay.
- **Full VCV-style CV model (per-sample control voltages everywhere)** — powerful but overkill; the typed-kind model (audio/control/trigger/note) covers the need and matches the corpus sketch. Deferred.
- **MIDI as the internal event format** — bakes a tempered grid into the substrate (the musical-event model note already chose continuous pitch). Rejected: MIDI is an external protocol, not the internal vocabulary.

## Consequences

- **Verified by tests (27, incl. 19 integration):** the patched chain renders scale-following pitch (zero-crossing frequency check — the pitch-semantics bug where `(pitch − 69)` treated semitones as MIDI notes is fixed); fan-in merges two producers sorted (a fire-at-100 + fire-at-6000 chain); type-mismatch / wrong-direction / unknown-port / unknown-plugin patches are refused and never logged; patch-before-mount is refused; patching is logged and replay byte-identical (including mid-session mounts, tempo changes, and a full unmount → re-mount proving plugin state resets); the counting-allocator tests hold with and without latency-bearing nodes (PDC preallocation).
- **Known limitations (deliberate, Spike A.5):** control is single-driver; the master out is mount-order dependent (last audio provider wins the bus — the Phase 1 mixer owns it); PDC compensates audio paths only, and `latency()` is per-node not per-port — record this before a CLAP plugin reports real latency; `Graph::set_param` is unlogged and unreachable via `Engine` — expose it with a logged event in Phase 1 or delete it; `EventBuf` / `MAX_PENDING` drops are silent — make them observable counters in Phase 1; apply-time refusals are logged-as-intent; control-side mutations (mounts, patches) allocate on the render call stack in Spike A.5 — a real control→render handoff is Phase 1; voice management stays deferred (RESEARCH §14 risk 8).
- **Next:** Spike B — cpal device path, disk streaming, the recording writer. The patch bay is the seam external I/O plugs into, and `providers_of` is the interface the patch UI renders.
