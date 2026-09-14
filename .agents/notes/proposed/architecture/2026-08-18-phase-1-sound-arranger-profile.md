# Agent Note: Phase 1 — the sound-arranger profile (engine-side plugins)

Status: proposed

## Problem

Phase 0 proved the core and the media engine; RESEARCH §11 defines Phase 1 as the first assembled profile: **recorder plugin** (cpal → media pool, live peaks) + **clip-editor plugin** (cut/paste, razor-split, trim, copy/duplicate, crossfade, loop regions) + **soft mixer plugin** (gain/pan/mute/solo, master fader, meters, bounce to WAV). Two Spike-B core-shape findings block it: (1) **media/param changes are not logged** — `Graph::set_param` is unlogged and unreachable via `Engine`, and the media mailbox bypasses the log, so profile-level edits cannot be replayed byte-identically; (2) **nobody owns the master bus** — the master out is mount-order dependent (Spike A.5), and the graph's "one port per kind per direction" constraint cannot express a mixer's per-channel inputs. This note owns the Phase-1 shape and order; per-step implemented notes own the shipped reality.

## Proposal

Phase 1 runs in the engine workspace (no Tauri — the canvas/drag UI waits for the app shell; RESEARCH §11). Order decided 2026-08-18 (user): **foundation + mixer first → recorder → clip-editor**.

1. **P1.0 Foundation — logged `SetParam` + the mixer owns the master bus:**
   - **Logged `SetParam`**: `Event::SetParam { plugin, param, value, at_frame }` + `Engine::set_param` — validated fail-loud (plugin must be mounted), logged, applied by the render loop at its frame via the scheduler (sample-accurate, replayable). This is the generic, model-free control path the mixer, recorder, and clip-editor all use. `SetParam` is a **discrete** change (a mute toggle, a gain step); automation curves (fader rides) are a later log event type (minimal-core note's log rules: control-rate streams coalesce into gestures/curves — the render side interpolates; not Phase-1 scope).
   - **Graph generalization**: audio `In` ports may be multiple per node (the mixer's `ch0..chN`); patch cords carry **port indices**; the interpreter sums producers per input port (`NodeIO.audio_ins`), keeping `audio_in` as the first port for single-input nodes. Control/trigger/note stay one-port-per-kind (Phase-1 scope; the mixer needs no event inputs). All buffers preallocated — the no-alloc render invariant holds.
   - **The mixer owns the bus**: on mount, the mixer sets `out_node` (the A.5 "last audio provider wins" limitation is gone); sources route into its channels via normal patch cords.
2. **P1.1 Soft mixer** (`engine::plugins::mixer`): N audio input channels, per-channel `gain`/`mute`/`solo`, `master.gain`, per-channel + master **meters** (peaks per block, exposed on shared atomics — the render path writes, the control side reads; no blocking), and **bounce to WAV** (render the master + write 16-bit WAV — float on disk is the media *pool*'s format, 16-bit at bounce per the prior-art research). **Mono** in Phase 1 — pan needs a stereo bus, deferred with the stereo step (the Phase-0 graph is mono, documented).
3. **P1.2 Recorder plugin** (`crates/media`, registered into the engine): cpal input → drift compensator → **float-WAV media pool** (32-bit float sources, content-hash referenced, opaque managed pool — the prior-art disposition) + **live peaks** (the 256-sample min/max reduction pyramid, background-computed, persisted sidecar — the Audacity structure).
4. **P1.3 Clip-editor plugin** (engine/media): the ACID operations as logged commands — razor-split, trim, cut/paste, duplicate, crossfade, loop regions — applied at their frames; Spike B's splice is the seed. The largest step; its shape (a clip timeline as graph value vs. per-clip nodes) is a dedicated note when it starts.

## Alternatives considered

- **Recorder-first** — the EP-133 jam loop sooner, but the recorder routes into the master bus, so a minimal bus would be built twice. Rejected; the mixer is the smallest plugin and unblocks everything.
- **Clip-editor-first** — the ACID heart on the current mount-order bus, deferring the mixer; the bus limitation would leak into every edit test. Rejected.
- **Per-sample/continuous `SetParam` logging (fader rides as thousands of events)** — violates the minimal-core note's log rules (control-rate streams coalesce). Rejected; `SetParam` is discrete, curves are a later event type.
- **Mixer as a core concept (the bus as engine machinery)** — couples the model-free core to the mix domain; the mixer is a plugin like any other, it just *claims* the master out on mount. Chosen.
- **Pan now (stereo mixer)** — the graph, media, and WAV modules are all mono; stereo is a cross-cutting change better done as its own step. Deferred.

## Acceptance criteria

- `Engine::set_param` is logged and replayable: two fresh engines with the same log render byte-identical output including mid-session param changes; refused params (unmounted plugin) are never logged.
- A mixer with two sources on ch0/ch1 and different gains produces the gain-weighted sum at the master; mute silences its channel; solo makes only solo'd channels sound; `master.gain` scales the sum; meter atomics hold the expected peaks after render; the mixer is the master out (sources do not become the bus).
- A param change at frame F applies sample-accurately (block-split), like mounts.
- Bounce writes the master to a readable WAV that round-trips the rendered output.
- All preallocated: the counting-allocator test covers a mixer graph.

## Risks

- **Graph generalization regressions** — per-port routing touches the hot loop; the engine's existing no-alloc and determinism tests guard it, plus new mixer tests.
- **Solo/mute semantics** — solo interacts across channels; the rule (any solo ⇒ only solo'd channels) is stated and tested.
- **The mailbox stays for media commands until the recorder step** — the
  recorder/clip-editor absorb it into logged events; the phase note tracks it.
  **Shipped 2026-09-12:** `pool`/`play`/`splice` are logged media ops — see the
  [media-commands note](../../implemented/architecture/2026-09-12-media-commands-logged.md);
  the mailbox remains the reader's internal buffer.
