# Agent Note: Software engine + hardware interface; vertical (Cardinal) and distributed (network) scaling

Status: proposed

## Problem

Prototyping the planned Eurorack build in VCV Rack (see [the rack plan](../../../../research/gear/behringer-eurorack-go-rack-plan.md) and the [vcv-patch sandbox](../../../../vcv-patch/README.md)) surfaced a clear split: **software wins on extensibility and interoperability; hardware wins on tactile UX** (turning knobs with a mouse is not a good "live" experience). And the audio engine is **CPU-bound** — a single machine's CPU/RAM is the ceiling, and the owner's machine is "upper-midrange." We need two things: (1) a way to use physical hardware as a *tactile* interface to the software, and (2) a way to **scale the DSP** past one machine's CPU/RAM when we hit that ceiling. Because the arranger **records long takes** and is *not* in the live-monitoring path, scaling is a different (more forgiving) problem than for a live performance tool.

## Proposal

**Software = engine; hardware = interface. Converge, don't choose.** The sound-arranger engine (with an embedded modular synth) is the extensible core; physical hardware is the tactile control surface. Map hardware to the [input/jam-layer](2026-09-01-input-jam-layer-device-registry-io.md) control-surface abstraction (control verbs / `set_param`), not to a mouse. VCV Rack's MIDI map, the 0-dimension XY/pads, and later the Eurorack itself (real knobs, joystick) all drive the same abstract controls; the NTS-3 XY pad (class-compliant MIDI CC) is a working example today.

**Vertical scaling — embed Cardinal as CLAP plugin instances.** Cardinal (falkTX/Distro) is the free, open-source virtual modular synth, VCV-compatible, that runs standalone **and as an LV2/VST3/CLAP plugin**. In the engine, mount **multiple Cardinal instances as opaque `AudioNode`s** via the existing **`clack` CLAP host** — this is the already-planned [integration route A](../../../../research/architecture/2026-08-19-softsynth-integration-routes.md). Each instance = a self-evolving patch / voice / sub-graph; the graph interpreter renders them **in parallel across CPU cores**. This is how one machine scales: *N Cardinal instances across N cores*, each a self-contained modular voice — no need to write our own modular synth, Cardinal is the embeddable host.

**Horizontal scaling — distributed (later, and latency-tolerant).** Distribute sub-graphs across networked machines, exchanging audio/CV over network audio (open **NetJack** first; Dante/AVB/virtual-soundcard as alternatives), sync via MIDI clock / drift-compensated transport. This rides on the **graph-as-value + deterministic session-log** architecture: serialize → partition into sub-graphs → run on workers → record each machine's audio. Because the arranger *records*, network latency is tolerable.

**Be clear-eyed about the hard parts (all deferred, not blockers):** sample-accurate cross-machine sync is hard (the existing DriftCompensator helps); floating-point determinism is per-machine (across machines we rely on *recorded audio*, not re-synthesis — fine, the arranger records); latency is fine for recording but bad for live monitoring; a real distributed system is a big lift.

## Alternatives considered

- **Write our own modular synth engine** (fundsp blocks, own graph) — still worth a few native voices per the [native soft-synth note](2026-08-30-native-soft-synth-building-blocks.md), but embedding Cardinal via `clack` gets a whole modular host for ~zero DSP-authoring work. Rejected as the *primary* vertical-scaling mechanism (native voices remain a complement).
- **VCV Pro's paid DAW-plugin feature** — closed/paid; Cardinal is the open, free equivalent. Rejected.
- **A single VCV Rack instance relying on more threads** — VCV Rack's audio engine is effectively single-threaded per patch, so one instance doesn't use a multicore machine; hence **multi-instance**, not one big instance. Rejected as the only answer.
- **Proprietary AoIP (Dante) for horizontal** — locked/expensive; prefer open NetJack / latency-tolerant multitrack-over-LAN first, AoIP as an option. Rejected as the default.
- **Build the distributed model now** — premature; defer (see Risks).

## Acceptance criteria

- The engine mounts **one or more Cardinal instances as opaque audio nodes** (via `clack`), renders them in parallel across cores, and the arranger records/mixes the result — *vertical scaling proven on a single machine*.
- A **hardware control surface** (MIDI controller now; Eurorack + NTS-3 XY later) drives the control-surface abstraction, not a mouse — the "software engine + hardware interface" formula working end-to-end.
- The **session log + graph interpreter** remain the substrate; the **distributed (network) model** is documented as a future phase, not the critical path.

## Risks

- **Cardinal is VCV Rack 1-API-based; Rack-2-only modules may not load in Cardinal** (e.g. the Rack-2 modules in the current patch). Mitigate by using module builds that are VCV-1-compatible, and by treating Rack 2 (standalone) as the authoring sandbox vs. Cardinal (embedded) for the subset that loads. **Verify per module before relying on Cardinal as the embed path.**
- **Licensing:** Cardinal is GPLv2; embedding it as an opaque node via a CLAP host keeps it a separate in-process plugin (like the CDP8 sidecar pattern) — confirm no copyleft taint on the GPL-or-later app (§12).
- **Multi-instance memory:** each Cardinal instance is a full synth, so vertical scaling costs **RAM**, not just CPU — measure with [log-vcv-usage.sh](../../../../scripts/log-vcv-usage.sh).
- **Complexity/deferral:** distributed is a real lift; keep it a later phase and don't let it inflate the critical path.

---

*Authored with DeepSeek-V4-Flash · deepseek-harness, 2026-09-03.*
