# Agent Note: The mixer's width is a mount parameter, not a property of the code

Status: implemented

## Problem

The mixer's channel count was a **compile-time** property in five places: the cap
`MIXER_CHANNELS_MAX = 8`; the hand-written `MIXER_PORTS`/`MIXER_PARAMS` tables; the node's fixed
arrays (`gains: [f32; N]`, `mutes: [bool; N]`, …); `MeterBank([AtomicU32; N + 1])`; and, outside
the engine, `capture.rs`'s `1..=8` clamp and the live host's `[f32; N]` meter snapshot. A mount
parameter could pick a width *below* the constant and nothing could pick one above it.

The recorder's workflow is **additive dubbing** — a pass per lane, one instrument at a time — so
lanes grow with passes rather than with instruments, and eight is reached early. The ceiling was
therefore the product's limit, not a tuning knob. It was also a device-shaped assumption: the
platform's other sources (VCV, SuperDirt, Tidal, a second instance of ourselves) have no device to
report a channel count from.

## Decision

**Everything about the mixer's width is derived from the mount.**

1. **The surface is generated**: `channel_ports(n)` / `channel_params(n)` build the ports and
   parameters of the instance that mounted. Their names cannot be literals any more, and a
   `Port`/`ParamDef` carries `&'static str`, so names are **interned** — once per *name per
   process*, not once per mount, because a replay rebuild re-mounts and per-mount leaking would
   grow without bound.
2. **The node's state, its meter bank and its per-block peak scratch are allocated at mount.**
   The render path still allocates nothing: the scratch is a field, cleared and reused per block,
   which the counting-allocator test keeps honest.
3. **`MIXER_CHANNELS_MAX` is gone.** What remains is a **named sanity bound**
   (`MIXER_CHANNELS_SANITY`, `CAPTURE_CHANNELS_SANITY`, both 64) with a stated reason: a typo guard
   so `channels=1000000` cannot allocate gigabytes. Nothing is sized by it and it is never an array
   dimension.
4. **The engine resolves a plugin's surface in three rungs**: the applied instance's mounted
   surface, then the **queued mount's factory** — asked with the params it will be mounted with,
   which is the call `validate_mount` already makes — then the registered catalog. The middle rung
   is what keeps `mount` followed by `patch` in one script working now that the width comes from
   the mount params.
5. **The static catalog stays, nominal** (`ch0..ch7`): it is the patch bay's data and the
   pre-mount fallback. It is no longer what validation believes.

## Alternatives considered

- **Raise the constant (8 → 32 or 64).** Rejected: it answers "how wide can a session be?" with a
  second guess, and (with this engine) it must stay an array dimension, which is the structural
  part of the ceiling.
- **Macro-generate a wide static catalog.** Rejected: 64 literal ports and 257 literal parameters,
  and still a ceiling — just a bigger, better-hidden one.
- **Leak the generated names per mount.** Rejected: mounts repeat on every replay rebuild, so the
  leak grows with use rather than with the widest layout.
- **Change `register_factory` to take owned surfaces.** Not needed, and deferred: with the catalog
  nominal, the 35 registration sites stay as they are. Revisit if a profile ever *declares* a
  catalog wider than the nominal one.
- **A small-vector dependency for inline storage.** Rejected: `crates/engine` is std-only, and the
  buffers here are ordinary `Vec`s allocated on the control side.

## Consequences

- A mixer mounts at any width up to the bound **with no code change**. Proved twice: a 20-channel
  mount patched on both rungs (queued and applied — 20 used to be impossible), and a graph test
  where twenty sources reach twenty mounted channels and the twentieth channel's meter reads.
- The catalog can disagree with the mounted surface **by design**, and the factory rung is what
  makes that safe for a queued mount. The catalog is a promise about *shape*, never a promise about
  width.
- The intern table is a process-wide, bounded allocation. It is the price of `&'static str` names
  for a surface that is no longer static, and it is stated at the function that does it.
- The sanity bound is the honest residue of the ceiling: **named, documented, and not structural.**
  A rig that needed more than 64 lanes would raise a number, not restructure anything.
- Verification turned up a **pre-existing** flake:
  `live_host_executes_commands_and_refusals_survive` read the live snapshot immediately after a
  mount and raced the actor's publish (1 failure in 8 runs, reproduced on the unmodified tree). It
  is fixed the same way the documented shell-snapshot flake was — waiting for the state instead of
  racing it — and then passed 8 of 8.

*Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-27.*
