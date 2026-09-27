# Agent Note: A node's input width is its own, not a constant

Status: implemented

## Problem

`MAX_AUDIO_INS = 8` was not merely a validation bound — it was the **array dimension**
in the node-facing struct: `NodeIO { audio_ins: [&'a [f32]; MAX_AUDIO_INS] }`, filled by a
per-block loop in the graph's render path, and asserted at `add_node`. Every node's input view was
therefore fixed at compile time, and a node declaring more than eight inputs was refused.

That is the structural half of the channel ceiling the
[capacity note](../../proposed/architecture/2026-09-27-capacity-scales-with-the-rig.md) removes: no
mount parameter could widen a node's inputs while the *view itself* had a constant width. The
mixer's own `MIXER_CHANNELS_MAX` and its hand-written static port/param surface are the other half,
still open.

## Decision

**The node's input view is sized by what the node declared.**

- `NodeIO.audio_ins` is an `AudioInputs<'a>` — a view onto the graph's per-port fan-in buffers,
  read through `count()` and `get(port)`, with no maximum. `get` returns `&[]` past the declared
  width, so a node cannot index its way out of its own surface.
- The graph precomputes a per-node channel list (`audio_in_channels`) when a node is created, so
  the render path sizes the view **without touching the node's ports** — the buffers stay
  preallocated, and the render path still allocates nothing.
- `audio_in` and `audio_in_count` remain as the single-input convenience, filled **from the same
  view** (`ins.get(0)`, `ins.count()`), so the convenience and the view cannot disagree where the
  engine builds them.
- The ceiling assert and the `.min(MAX_AUDIO_INS)` clamp are gone, and the test that asserted the
  refusal is replaced by one proving the opposite: a node declaring **12** audio inputs reaches all
  twelve at render (`a_node_may_declare_more_inputs_than_the_old_ceiling`).

## Alternatives considered

- **Keep the array and generate a bigger one with a macro.** Rejected: it answers "how wide can a
  session be?" with another constant, which is the whole thing being removed.
- **A small-vector type with inline storage and a heap spill.** Rejected: it needs a dependency,
  and `crates/engine` is deliberately std-only.
- **A per-node scratch `Vec<&[f32]>` rebuilt each block.** Rejected: either it is a graph field
  borrowing its own buffers (self-referential) or it allocates per block, which the counting
  allocator forbids.
- **Hand node authors the raw `&[Vec<f32>]` plus channel counts.** Rejected: it exposes an
  allocation-capable type in the node API for no gain over a view that is `Copy` and needs no
  construction by the node.
- **Keep the convenience fields out of `NodeIO` and make them methods.** Right in principle — one
  source of truth — and deferred as churn: it would touch every `io.audio_in` reader plus the
  render stubs. The fields are now derived from the view, so they agree where it matters.

## Consequences

- A node's input width is its own; the graph has no fixed-width node surface left.
- The render path is unchanged in its allocation behaviour, which is what the counting-allocator
  test keeps honest.
- `audio_in_channels` is a **new parallel per-node vector**, and the lifecycle of those vectors is
  a contract with three parts: add, insert, and remove. Testing caught exactly this — the first
  version hooked add and insert but not `remove_node`, so removing any node shifted the channel
  lists and a later node read the wrong slice lengths. It surfaced as silence in the host's
  arranger test (`edit_to_a_wired_track_continues_the_clip_not_restarts_it`), not as a panic.
  Recorded because the next parallel vector will have the same three parts.
- Render *stubs* in tests pass `AudioInputs::none()` while setting the convenience fields directly.
  They only read the convenience, so the wart is inert — but it is a wart, and the deferred
  alternatives above are how it goes away.
- The remaining half of the ceiling is the mixer's own surface: `MIXER_CHANNELS_MAX`, the
  hand-written `MIXER_PORTS`/`MIXER_PARAMS`, and the `1..=8` clamp in `capture.rs`. Those need the
  surface to be the instance's — dynamic names, so `Port.name`/`ParamDef.name` stop being
  `&'static str` — which is a 58-site and 44-site sweep, sized and left as the next step.

*Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-27.*
