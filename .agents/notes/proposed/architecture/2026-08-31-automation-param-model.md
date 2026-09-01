# Agent Note: Automation model — a shared curve type + one smoother, riding the declared param surface

Status: proposed

## Problem

Params today are **discrete jumps**. `Engine::set_param` validates against a plugin's declared `ParamDef` surface, logs an `Event::SetParam`, and schedules a `SchedEvent::SetParam` that `apply_event` routes via `graph.set_param(node, param, value)` at its frame (sample-accurate, but a step). The engine's own comment says *"automation curves are a later event type."* This is the gap the driver/user hit when reasoning about the `tone` plugin growing a filter + LFO: a real voice and a fader sweep both need **continuous** control over time, not a jump. The kimi minimal-core review already flagged the requirement — *"parameter/automation model — smoothing primitives and automation lanes. Every profile needs them; if they're per-plugin, you get four zipper-noise implementations and inconsistent feel"* — and the native-soft-synth note names the concrete case (the fundsp voice's `cutoff`/`q` are baked at mount and refused by `set_param`; a real synth wants cutoff automation).

## Proposal

One automation model, not per-plugin, that reuses the param machinery already here:

1. **A curve value type.** `Automation { plugin, param, mode, points: Vec<(frame, value)> }` with interpolation `Linear` (default) or `Step`. Validated fail-loud: finite values, `frame`s non-decreasing, and every `value` within the plugin's declared `ParamDef` min/max (reusing `set_param`'s range check). It lives in `crates/engine` as a plain value (like `Value`/`ArrangeOp`), because it must cross into the log and replay.
2. **One shared smoother.** A control→render ramp: between two consecutive curve values, the param is driven over a short ramp window (the per-param `smoothing` seconds/`ParamDef.smoothing` default) rather than stepped, so a swept param does not zipper or click. No allocation on the render path (reads preallocated curve slices + a scalar).
3. **It rides the existing surface.** The curve applies through the declared `ParamDef` namespace and `graph.set_param(node, param, value)` — so **any** plugin param becomes automatable for free; the engine owns the interpolation, the plugin just declares its params as it already does.
4. **Logged and replayable.** New `Event::AutoParam { plugin, param, curve, at_frame }` (and matching `SchedEvent`), pushed to the log and applied by the render loop; `replay_from` re-schedules it so the timeline reconstructs byte-identically. Refused (invalid) curves are never logged, matching `set_param`.

The **wiring** (how `apply_event` converts a curve into a sequence of ramped `graph.set_param` calls and where the ramp state lives) is a clearly-scoped follow-on once the criteria below hold. The spike is small and stays in `crates/engine` — feature-gated (`automation`) so the minimal core stays dep-lean, or always-on if it proves small enough.

## Acceptance criteria

- **AC1 — interpolation is correct:** `Automation::value_at(frame)` returns the right value; `Linear` ramps between breakpoints, `Step` holds, ends clamp (before-first → first value, after-last → last value).
- **AC2 — no click:** sweeping a real param (e.g. `mixer master.gain` over ~N frames) produces contiguous output (no step discontinuity at any block boundary; the smoother bridges within a bounded ramp). No `NaN`/inf.
- **AC3 — zero-alloc render:** the sweep path does not allocate on the render stack (counting-allocator test), consistent with the engine's no-alloc invariant.
- **AC4 — byte-identical replay:** a fresh engine replaying the same log with the same curve renders byte-identically to the live run (and the refused-curve path is never logged).

## Alternatives considered

- **Per-plugin automation** (each voice owns its own lanes). Rejected — kimi's exact objection: four zipper-noise implementations, inconsistent feel, and it can't compose across the patch bay.
- **Jump-only (as today).** Rejected — that is the gap; the moment a filter/LFO or fader ride exists, a step is audible.
- **MIDI CC as the automation source.** That is the control-surface path (the NTS-3 XY pad → CC per the gear research) and is a *source*, not the model; it still needs the curve+smoother to be recorded as the same value type. Deferred, separate.
- **Reuse the scheduler with many `SetParam`s** (a dense per-frame sweep). Rejected — O(frames) events, not a value, defeats the "curve is data" and byte-identical-replay goals.

## Risks

- **Realtime/no-alloc:** interpolation and smoothing must be O(1) per block, no allocation, no `Vec` in the render path (curve slices are read-only; ramp state is scalars). AC3 guards this.
- **PDC:** the ramp target must be applied at the frame the curve says, in the same phase as the audio it affects — automation is not a reason to re-open PDC, but the sweep must be frame-consistent.
- **Log versioning:** a new `Event` variant means `replay_from` and any existing log consumers must handle it; older logs replay unchanged (no events of the new kind), and the schema stays append-only.
- **The smoother is a codebase-wide knob** — a single default so feel is consistent, per-param override available, never four uncoordinated ones.

## Forward directions recorded (not this spike)

- **Softsynths are profiles, defined not hardcoded.** A "profile" is a declaratively assembled patch — a composition value (mount/patch/param wiring) — which is exactly the patch-bay-as-value model (composition-seams) plus the native-soft-synth decision (fundsp supplies DSP blocks; our `AudioNode`/patch stays the loggable value). Automation rides the same profile values, so a voice's filter/LFO sweeps are part of the profile, not baked in.
- **A symbolic layer (Pd/Max-like) is a future direction.** Expose the value-based patch bay to users as a patcher/graph DSL, eventually over the same values — a later phase, not part of the automation spike.
