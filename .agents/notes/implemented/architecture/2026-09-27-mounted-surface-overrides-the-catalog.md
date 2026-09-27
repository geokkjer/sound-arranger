# Agent Note: The mounted surface overrides the catalog — a patch to an unmounted channel is refused

Status: implemented

## Problem

Validation read the **registered catalog**, not the running instance. `Engine::register_factory`
captured a plugin's `ports`/`params` into `port_table`/`params_table`, and both `validate_patch`
and `set_param` consulted those. But the mixer's width is a **mount parameter**: it registers
`ch0..ch7` and may mount two of them.

So a patch to `mixer.ch5` on a two-channel mixer validated happily against the catalog's `ch5`,
and was then silently ignored at render — the documented "accepted but ignored" path. The
[channel-layout note](../../proposed/feature/2026-08-20-channel-layout-typed-value.md) called this
out as "a silent behavior, the opposite of the patch bay's fail-loud rule", and the
[capacity note](../../proposed/architecture/2026-09-27-capacity-scales-with-the-rig.md) makes its
removal an acceptance criterion.

## Decision

**Validation asks the instance what it mounted.**

- `Plugin::mounted_ports()` / `mounted_params()` answer for the instance; they default to the
  declared surface, so a plugin with a static surface says nothing.
- `Engine::apply_mount` captures them keyed by plugin name, `apply_unmount` drops them, and
  `validate_patch`/`set_param` **prefer** them, falling back to the catalog.
- The mixer's `mounted_ports()` returns only the channels it mounted plus the stereo master out —
  and `apply` adds its node with **that** surface rather than the catalog's. So the graph itself
  refuses a patch to an unmounted channel: the loud rule is structural, not merely validated, and
  the render-side `min(count, channels)` guard becomes defense in depth rather than the only check.

## Alternatives considered

- **Let the node refuse at render.** Rejected: a node never sees patch requests, and by render time
  the silence is already the behavior being complained about.
- **Keep the catalog authoritative and give the mixer a static maximum.** Rejected: that is the
  channel ceiling this work exists to remove, and it is why the catalog alone cannot be right.
- **Special-case the mixer in `validate_patch`.** Rejected: a plugin-shaped hole in the core, and
  the next multi-channel plugin would need its own.
- **Drop the catalog now — every plugin instance-provided.** Right eventually, and deferred for a
  concrete reason: a **scheduled** plugin has no instance yet, so a patch between `mount` and the
  frame that applies it has nothing to ask. The catalog is that fallback, and it stays until the
  surface itself is dynamic (below).
- **Make the node's declared ports the catalog's, keeping both in sync by hand.** Rejected: two
  hand-maintained lists of the same fact is the drift this replaces.

## Consequences

- A patch to an unmounted channel fails with `no port 'ch5' on plugin 'mixer'`, and the test that
  documented the silent path now documents the node's own guard instead.
- The catalog and the mounted surface can disagree **by design**, and only in one direction: the
  instance may be narrower. The fallback covers the scheduled window, which is the sole reason the
  catalog still exists.
- **The ceiling is not gone.** Names are still `&'static str`, so the mixer's own
  `MIXER_CHANNELS_MAX`, the hand-written `MIXER_PORTS`/`MIXER_PARAMS`, its fixed-size state
  (`[f32; MIXER_CHANNELS_MAX]`) and `MeterBank([AtomicU32; N + 1])`, and the `1..=8` clamp in
  `capture.rs` all remain. This slice removes the *silent* path; removing the *ceiling* needs the
  surface to be generated from the mount layout, which needs owned (or interned) names — the step
  the [node-width note](2026-09-27-node-input-width-is-the-nodes-own.md) also names.

*Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-27.*
