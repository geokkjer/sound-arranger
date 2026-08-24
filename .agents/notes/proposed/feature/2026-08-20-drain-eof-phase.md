# Agent Note: Drain/EOF phase — stateful nodes must emit their buffered tail at the end

Status: proposed

## Problem

The render loop ends when the frame budget is exhausted: `Engine::render_into` chunks the output buffer into fixed `BLOCK`s and stops at the last frame ([`render.rs`](crates/engine/src/render.rs)) — a **hard cut**. Any node that buffers output internally (a delay/reverb tail, a granular voice, a lossy codec's encoder delay, a waveshaper with lookahead) holds samples that are simply never emitted. Today the tone's fixed-decay blips mostly expire inside a block, so the bug is latent; the moment a tailed effect or a codec lands, "bounce the arrangement" silently truncates audio that is musically and technically part of the piece.

FFmpeg's send/receive reform names the correct shape: stateful producers need an explicit **drain** — *"the encoder or decoder requires flushing with NULL input at the end in order to give the complete and correct output"* (`AV_CODEC_CAP_DELAY`), implemented as send-`NULL` → receive until `AVERROR_EOF` → `avcodec_flush_buffers()` ([ffmpeg research](research/architecture/2026-08-20-ffmpeg-design-knowledge.md)). Our offline bounce (and any future `OfflineProcess` export) has no such phase, and our byte-identical-replay promise makes the missing tail a correctness bug, not a nicety: the bounce is a different — truncated — output from what the session "is."

## Proposal

Add an explicit **drain/EOF phase** to the graph render path, mirroring the send/receive flush protocol:

- **A node-level `drain()` capability** on the opaque-node trait (default: no-op). Nodes with internal buffering declare it — the "I hold output" capability, the analogue of `AV_CODEC_CAP_DELAY`. `ToneGen` gains it once its blip pool can outlive a block; future reverb/delay/granular nodes are written against it from the start.
- **`Engine::render_into` gains an optional drain step**: after the timeline's last frame, render drain-mode blocks — each node's `drain()` emits its buffered tail into the block until it reports done, or a **bounded iteration cap** is hit (feedback structures can ring forever; the cap plus a declared max tail length makes drain terminating). Hitting the cap is a logged, fail-loud warning, not a silent truncation.
- **Realtime transport stop uses the same drain path** (possibly with the existing fade discipline) so stopping playback and bouncing end the piece the same way; drain output is deterministic, so replay stays byte-identical.
- **The drain policy is logged** as part of the bounce/`OfflineProcess` event (`drain_tails` vs `hard_cut`) — a bounce with tails is a different event than one without, and replay must reproduce the same one.

## Alternatives considered

- **Status quo (hard cut at the last frame)** — silently truncates any buffered tail; a latent correctness bug that becomes real with the first tailed effect. Rejected.
- **Extend the timeline by N seconds of silence so tails ring out** — arbitrary N, changes timing semantics, and doesn't cover encoder delay or lookahead (which are *inside* the signal, not after it). Rejected.
- **Drain only at export (let the ffmpeg sidecar handle its own codec delay)** — covers codecs but not our own nodes' tails, which are the bigger musical loss. Rejected: the phase belongs in the graph, and the sidecar handles only its own codec delay.
- **Render extra blocks until a silence threshold** — nondeterministic (depends on tail energy), breaks byte-identical replay; a threshold is a policy on top of drain, not a replacement for it. Rejected.
- **Only nodes that need it self-drain inside their `render`** — requires nodes to know the timeline has ended, coupling them to the scheduler. Rejected: drain is a distinct graph-level phase, like ffmpeg's send-NULL signal.

## Acceptance criteria

- A node with a declared drain capability emits its full buffered tail after the timeline ends; a `ToneGen`-class node with a pending blip pool drains it.
- Offline bounce including drain is byte-identical across replays; the bounce event records the drain policy.
- Drain terminates: bounded by the iteration cap or declared max tail length; hitting the cap logs a warning and fails loud rather than truncating silently.
- Realtime transport stop exercises the same drain path as bounce.
- A new tailed effect (delay/reverb/granular) written after this note ships can be drained without render-loop changes — the capability is on the trait from the start.

## Risks

- **Unbounded feedback structures** (delay with high feedback) never converge — bounded by the iteration cap and declared max tail; the cap is a logged, explicit failure mode, not silent loss.
- **Performance**: drain adds post-timeline blocks to every bounce — bounded by the same cap, and only when a mounted node declares drain; realtime playback pays nothing unless a tail is actually ringing.
- **PDC interaction**: drain runs after the PDC-compensated end; a node whose latency overlaps the tail must not emit into a stale window — drain starts at the compensated end frame and is tested against the PDC suite.
- **Log growth**: drain policy is one field per bounce event, not per block — negligible.
