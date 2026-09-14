# Agent Note: Drain/EOF phase — stateful nodes emit their buffered tail at the end

Status: implemented

## Problem

The render loop ended when the frame budget was exhausted: `Engine::render_into`
chunked the output buffer into fixed `BLOCK`s and stopped at the last frame — a
**hard cut**. Any node that buffers output internally (a delay/reverb tail, a
decaying voice, a granular pool, a codec's encoder delay) held samples that were
never emitted. The bug was latent while the tone's fixed-decay blips mostly
expired inside a block, but the moment a tailed effect or a codec landed,
"bounce the arrangement" would silently truncate audio that is musically and
technically part of the piece — and the byte-identical-replay promise made the
missing tail a correctness bug, not a nicety.

## Decision

The graph render path has an explicit **drain/EOF phase**, mirroring FFmpeg's
send-NULL / receive-until-EOF protocol (`AV_CODEC_CAP_DELAY`).

- **`AudioNode::has_tail(&self) -> bool`** is the "I hold output" declaration
  (default `false`). `ToneGen` reports it while a blip is sounding; `FundspSynth`
  while its envelope is live. **Contract:** during a drain it must be *monotone
  non-increasing* — that is what makes the drain terminate — and a node reporting
  `false` emits no non-zero audio in a drain block.
- **`RenderMode { Timeline, Drain }`** on `RenderBlock` is the EOF signal
  (send-NULL). In `Drain`, a self-driven source mutes itself: `Sine` fills
  silence, `EuclideanGen` emits no onsets, and the ring-streaming media sources
  (`ArrangerNode`, `PlaybackNode`, `CaptureNode`) fill silence. Processing nodes
  (`Gain`, `Mixer`, and any tailed effect) render normally, so tails travel the
  chain to the master. Drain participation is therefore **explicit**, never
  inferred from a node's port shape.
- **`Graph::render` renders every node in index order**; `render_drain` is the
  same path with a `Drain` block and reports `has_tail()`. There is no separate
  DSP path for drain.
- **`Engine::drain(max_frames)`** renders `Drain` blocks after the timeline until
  no node reports a tail, then flushes the PDC transit still in flight, bounded by
  `MAX_DRAIN_FRAMES` (60 s at 48 kHz). It returns the tail audio and a
  `DrainOutcome { tail_frames, capped }`; `capped` is a fail-loud outcome, never a
  silent truncation.
- **The flush is independent of tails and correctly sized.** `Graph::flush_frames`
  computes the transit `T[i] = d_i + max over consumers c (latency[c] + T[c])`
  (`T = 0` at a node with no consumer), where `d_i` is the PDC delay applied to
  node `i` — `max_cum` alone under-counts chained paths. The flush is **re-armed
  at the tail→false transition**, so a tail longer than the transit is still
  followed by its flush, and it fires even when no node has a tail (in-flight
  samples are not "audio presence").
- **`DrainPolicy { HardCut, Tails }`** names the choice;
  `Engine::render_with_drain` is the offline shape. The reference host's `Bounce`
  uses `Tails`, treats a capped drain as a hard error, counts the drain against
  `MAX_BOUNCE_BYTES`, and reports `drain tail frames` in `summarize`.

## Alternatives considered

- **Status quo (hard cut at the last frame)** — silently truncates any buffered
  tail; a latent correctness bug that becomes real with the first tailed effect.
  Rejected. Kept as `DrainPolicy::HardCut`, the explicit legacy escape hatch.
- **A graph-level "render iff the node has an audio input or declares a tail"
  filter** — the first implementation. Rejected after review: it reinterprets port
  shape as drain lifecycle, so adding a sidechain/FM audio input to a free-running
  source flips it from muted to droning with no compile error. Replaced by the
  explicit `RenderMode`.
- **A separate `fn drain(&mut self, …)` trait method called instead of `render`** —
  a second DSP path is how "the drained mix sounds different" bugs are born.
  Rejected: drain reuses `render` with a mode.
- **Sizing the flush by `max_cum` (cumulative latency)** — under-flushes chained
  paths (a source feeding a latency node owes `d + latency`, not `d`). Rejected.
- **Computing the flush once at drain entry and burning it** — a tail longer than
  the transit reaches `has_tail() == false` with the flush already spent.
  Rejected: the flush is re-armed at the tail→false transition.
- **Rendering extra blocks until an energy threshold** — nondeterministic
  (depends on tail energy) and breaks byte-identical output. Rejected.
- **Content-trimming the trailing padding** — an epsilon/zero test makes the
  bounce *length* a function of DSP rounding, FTZ/DAZ and denormals, i.e.
  machine-dependent. Rejected: the ≤ `BLOCK`-1 padding stays.

## Consequences

- A drained bounce includes a stateful node's full tail. `ToneGen` and
  `FundspSynth` tails are covered by tests, and a `Sine → latency → master` graph
  proves in-flight PDC samples are flushed with no tail present.
- Drain is deterministic (a pure function of node state — no wall clock, no
  randomness), so `render_with_drain` is byte-identical across identical sessions.
- **Not yet shipped (explicit):**
  - the drain is **not a logged event**, so a session that bounces and then
    continues is a determinism boundary: the clock advances by `tail_frames` with
    no `Event` recording it, and `Host::seek_to` / later edits sit after the
    drain. This rides the **media-command logging** work (the arrangement ops are
    logged; `Bounce` and the other media commands are not).
  - **Realtime transport stop does not drain.** The live pump pauses the device and
    drops the ring's tail; ringing out into a paused device is a separate change
    (and stopping without ringing out *or* resetting node state can leave a stale
    tail that sounds at the new position on resume).
  - **The scheduler is ignored during drain.** An event scheduled past the
    timeline is skipped by the drain and applied by the next `flush_scheduled`; a
    terminal bounce should run when the scheduler has no future events.
  - `MAX_DRAIN_FRAMES` is a universal bound standing in for a per-node declared
    tail length; a node whose tail exceeds it is reported `capped`, not truncated
    silently.
- `has_tail()` scans the mounted nodes once per drain block — negligible.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-12.
