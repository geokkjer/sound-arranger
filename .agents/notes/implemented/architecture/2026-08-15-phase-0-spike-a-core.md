# Agent Note: Phase 0 Spike A — the minimal core ships (clock · graph · log · context + euclidean)

Status: implemented

## Problem

The minimal-core note (2026-08-15-minimal-core-clock-graph-session-log.md) defined four core pieces and Spike A as the first validation: prove the paradigm before scaffolding any UI. Nothing of the core existed; the acceptance criteria were hypotheses the spike had to falsify or confirm.

## Decision

The engine crate ships at `crates/engine` — std-only, no external dependencies, edition 2024. The four core pieces exist as modules, assembled by `render::Engine`:

- **`clock`** — sample-accurate master clock, an editable tempo/meter map (`TempoMap`: beats always *derived* from absolute frames, so tempo edits never corrupt positions), and a sample-accurate scheduling queue (`Scheduler`). No plugin owns time.
- **`graph`** — the audio graph interpreter: a value model (nodes / single-input edges / params) with two node tiers — declarative (`Sine`, `Gain`) and opaque (`AudioNode` trait; `BlipSynth` as the first opaque voice) — plus per-node latency with PDC (earlier stages delayed so paths align). Render is allocation-free: preallocated scratch, delay lines, and cumulative-latency vector.
- **`log`** — the append-only session log. **Every mutation is logged at call time with its absolute frame, then scheduled; the render loop applies it when the clock reaches that frame.** A refused mutation is never logged. `same log ⇒ byte-identical bounce`, enforced by tests including mid-session mounts and tempo changes.
- **`ctx`** — services, `inject` (fail-loud at mount), and reversible registration: `Plugin::apply` returns a `Disposer`; unmounting runs it (node removed, generator removed, provided service withdrawn). The clock is a *core* service satisfied by the engine (read-only via `PluginApi.clock`) — never a ctx clone (a snapshot would go stale).

Lifecycle is sample-accurate: the render loop splits blocks around scheduled events, so an unmount mid-block takes effect at its exact frame. The euclidean plugin (`plugins/euclidean`) is a pure maximally-even generator (Bresenham placement + rotation) that provides a `rhythm` service and mounts an opaque `BlipSynth` voice; a second test plugin injects `rhythm`, demonstrating spatial composability through a service key.

20 tests pass (8 unit + 12 integration), clippy clean. One is a counting-allocator test asserting the render path allocates zero bytes.

## Alternatives considered

- **cpal in Spike A** — a real device would have blocked determinism testing and the sandbox has no audio hardware. Rejected: render offline to a buffer; the device path is Spike B.
- **External dependencies (rtrb/basedrop/etc.)** — zero deps keeps the spike auditable and the no-alloc claim provable. Rejected for now; realtime handoff crates arrive with Spike B's device path.
- **Eager event application (apply at call time)** — kimi's review caught the frame-less-event hole: mounts and tempo changes without `at_frame` diverge on replay for mid-session changes. Rejected: everything goes through the scheduling queue.
- **Clock as a ctx service (clone)** — a clone freezes at frame 0; the inject check would pass but reads would be stale. Rejected: core services are satisfied by the engine, not ctx.
- **Eager unmount (block-quantized)** — kimi's review caught the block-early cut. Rejected: blocks split around scheduled events.

## Consequences

- **The public mutation API is the seed of the core plugin contract** (the future IPC command list): `mount`, `schedule_unmount`, `unmount`, `set_tempo`, `replay_from`, plus the `Event` type — the first written contract between host and engine.
- **Deferred, tracked where they belong:** the audio teardown protocol (ramp / voice-steal / tail-flush) — a hard cut is acceptable for a blip but the seam must exist before Spike B; event schema versioning — the log carries no version yet (add before it becomes durable in Phase 1); voice management (RESEARCH §14 risk 8); dynamic PDC latency changes (latency is fixed at mount); fan-out/mixing (single-input chain until the Phase 1 mixer); the unmount-before-mount cancellation edge (scheduler holds no cancel semantics yet); `Scheduler::pop` is O(n) (fine at control scale).
- **Spike B is next:** cpal device path, disk streaming, the recording writer, device-clock drift — the product's hard core, still inside the engine crate, no Tauri.
