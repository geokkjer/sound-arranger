# Agent Note: P1 foundation + soft mixer — logged SetParam, per-port audio fan-in, the mixer owns the master bus

Status: implemented

## Problem

Phase 0 proved the core and the media engine; the Phase-1 note (2026-08-18) defines the first profile. Two Spike-B core-shape findings blocked it: (1) **media/param changes are not logged** — `Graph::set_param` was unlogged and unreachable via `Engine`, and the media mailbox bypassed the log, so profile-level edits couldn't replay byte-identically; (2) **nobody owned the master bus** — the master out was mount-order dependent (Spike A.5: the tone claimed it), and the graph's "one port per kind per direction" constraint couldn't express a mixer's per-channel inputs. This step (P1.0 + P1.1, per the user's 2026-08-18 order decision) ships the foundation and the soft mixer.

## Decision

- **Logged `SetParam`** — the generic, model-free control path: `Event::SetParam { plugin, param, value, at_frame }` + `Engine::set_param`, validated fail-loud (registered plugin, **scheduled-or-mounted**, name declared in the plugin's parameter surface, value finite and in range), logged, applied by the render loop at its frame via the scheduler — sample-accurate and replayable; a refused call is never logged. The **`Plugin::params()` surface** (`ParamDef { name, min, max }`, registered in `register_factory`) closes the "unknown params logged as silent no-ops" hole the review found.
- **Per-port audio fan-in** — `PatchCord` now carries port indices + kind; `NodeIO` gained `audio_ins: [&[f32]; MAX_AUDIO_INS]` + `audio_in_count` (per audio-In port, preallocated scratch); audio `In` ports may be many per node, at most **one** audio `Out`, and one control/trigger/note port per kind. `MAX_AUDIO_INS = 8`, **enforced at `add_node`** (fail-loud). The first-node-wins master fallback now requires an audio Out (a trigger-only node can't become the bus).
- **The mixer owns the master bus** — `plugins::mixer`: 4 channels (`ch0..ch3` — the EP-133's four groups), per-channel `gain`/`mute`/`solo`, `master.gain`, per-channel + master **meters**, and mounting it claims the graph's `out_node`. The tone no longer claims the output (Spike-A.5 limitation gone); the spike tests now route through the Phase-1 chain (`tone.audio → mixer.ch0`).
- **Meters as a seam** — `MeterBank` (per-block peaks in shared atomics; render writes, control reads, no blocking). Meter points: **post-gain/pre-mute** per channel (a muted channel still shows its level), master post-fader; mute is absolute (wins over solo). The mixer provides the bank under the **`mixer.meters` context key** — the profile reads it via `ctx.get::<Arc<MeterBank>>("mixer.meters")` (the plugin-path reachability hole the review found).
- **Bounce** — `media::bounce`: render the master out to a 16-bit WAV (16-bit at bounce/export; the media *pool* keeps 32-bit float sources, per the prior-art research).

## Alternatives considered

- **Pan/stereo now** — the graph, media, and WAV modules are all mono; stereo is a cross-cutting change better done as its own step. Deferred (phase-1 note).
- **Per-sample/continuous `SetParam` logging (fader rides)** — violates the log rules (control-rate streams coalesce); `SetParam` is discrete, automation curves are a later event type.
- **The mixer as core machinery** — the bus stays a plugin that *claims* `out_node`; the core remains model-free. Chosen.
- **No `params()` surface (accept silent unknown params)** — erodes "refusals never logged"; the review called it a real invariant hole. A declared surface is the honest fix.

## Consequences

- **Verified by tests (14 Phase-1 + 1 media bounce):** `set_param` logged/replayable byte-identically, sample-accurate at the exact frame, refused-and-never-logged for unknown plugin / not-mounted / undeclared param / NaN / out-of-range, and accepted for scheduled mounts; the mixer routes two sources with per-channel gains to the weighted sum (1e-6), mute/solo (incl. mute-wins-over-solo and pre-mute metering), the master fader, meters through both the node and the plugin (`mixer.meters` ctx) path; the mixer owns the bus (unmounting it silences the master though the tone still sounds); middle-node removal rewires cords coherently; >`MAX_AUDIO_INS` refuses loudly; the mixer render path allocates nothing (counting allocator); bounce round-trips the master.
- **kimi review (2026-08-18)**: no criticals; five majors + minors fixed — see [the review archive](../../../../research/architecture/2026-08-18-kimi-review-phase1-foundation-mixer.md).
- **Known limitations (Phase-1 scope):** mono (pan arrives with stereo); discrete params only (automation curves later); `MIXER_CHANNELS = 4` fixed and **baked into the log's param names** — widening is a log-format decision, not just a code change; `MAX_AUDIO_INS = 8`; meters are per-block peaks; a `SetParam` whose plugin unmounts in the same frame applies as a replay-safe skip (documented edge, consistent with apply-time refusals); `Graph::set_param` remains an unlogged escape hatch (doc'd).
- **Recorded for P1.3 (the clip editor):** the `(plugin, param)` `&'static str` namespace cannot address per-clip/per-region entities — decide string interning or a `target-id` on `SetParam` before the clip editor builds on the seam; the media mailbox should be absorbed into logged events.
- **Next:** P1.2 the recorder plugin (cpal → float-WAV media pool + the 256-sample peak pyramid), then P1.3 the clip editor.
