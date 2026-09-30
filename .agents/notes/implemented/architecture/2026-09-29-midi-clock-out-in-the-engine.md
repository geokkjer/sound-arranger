# Agent Note: Clock-out in the engine — ticks from the block's own frame

Status: implemented

## Problem

The recorder profile needs the session to drive `clock=follower` gear: MIDI clock at 24 PPQN plus
`Start` / `Stop` / `Continue`. [The design note](../../proposed/architecture/2026-09-29-midi-clock-out.md)
set the shape; this is the engine half of it, and it surfaced two problems the design had not:

- **A portless node cannot measure its own block.** A clock generator has no audio ports, so it
  receives an empty `out_audio` — and the engine **splits blocks around scheduler events**
  (`render.rs`: "at their exact absolute frame by splitting the block around them"). Assuming `BLOCK`
  would therefore double-emit ticks across split boundaries.
- **The two services the plugin needs are both optional.** A plugin that *required* a sink or a
  transport log could not mount on a machine without a device, which is exactly the case replay has
  to survive.

## Decision

**`clock_out` emits the ticks due in the block it is given, from the block's own frame — nothing is
inferred from continuity, and nothing is required to be present.**

- `ExternalEvent` gains `Clock` / `Start` / `Stop` / `Continue`, each with `offset: u32` (the sample
  within the block), matching the existing variants. One event currency, both directions.
- **`NodeIO.frames`** carries the block width, because a portless node has no slice to measure. It is
  the cheaper home than `RenderBlock` (about ten literals to update against sixteen) and reads as what
  it is: something the node may read about this block.
- **Stateless tick math.** For each block, tick indices are derived from
  `TempoMap::beat_at(frame)` and each tick's frame is computed through `frame_at(n / 24.0)` — so f64
  noise cannot shift a tick, a tempo change inside a block stays exact, and a seek needs no special
  case: the next block simply computes from its own frame. The conservative decision (nothing sent
  across a rebuild) is implemented as *the absence of continuity tracking*, not as a rule to remember.
- **Services arrive through the context, and both are optional**: `"midi.out"` →
  `Arc<Mutex<Box<dyn MidiSink>>>`, `"transport"` → `Arc<TransportLog>`. `inject()` is empty, so a
  session with neither mounts and renders.
- **`TransportLog`** is sorted on push (a late feed arrives out of order — a test found it) and
  write-and-drain: entries due before the block's end are taken into a preallocated buffer and flushed,
  so a late transport command lands at offset 0 rather than being lost. The drain is bounded by the
  room that buffer has, so a flood of commands never grows it on the render path — what does not fit
  stays queued and flushes into the next block ([the drain-bound note](../../implemented/bug-fix/2026-09-29-the-transport-drain-is-bounded-by-the-scratch.md)).
  The *record* of transport is the session log, not this tap; the tap exists to get frames to the wire.
- **A loud bound on the scratch**: `CLOCK_OUT_CAP` events per block, with the rest counted through
  `overflows()`. Dropping clock is bad; allocating on the render path is worse; a bound that grows
  silently is worst. The cap is on the **send** scratch, and the tick **walk** carries its own bound
  ([the tick-walk note](../../implemented/bug-fix/2026-09-29-the-tick-walk-is-bounded-not-just-the-scratch.md)).
- **One `sink.send(&scratch, block.frame)` per block**, with the mutex held only across that call. A
  single sender means the lock is uncontended, and the real device sink (slice B) will make it a fast
  queue push so the render path never touches the device.

The test that carries the slice: **rendering the same session with and without a sink produces
bit-identical audio**, and the sink is separately shown to have sent. Whether the gear is attached
cannot change the mix.

## Alternatives considered

- **Assume `BLOCK` for the block width.** Rejected: the engine splits blocks at event frames, so a
  split block would emit a whole `BLOCK`'s worth of ticks twice. The bug would appear only in sessions
  with mid-block events — the worst kind to find late.
- **Add `frames` to `RenderBlock` instead of `NodeIO`.** Rejected on cost and reading: more literals to
  update, and the block descriptor does not otherwise carry runtime width to the node.
- **Require the sink.** Rejected: a mount that fails without hardware makes replay hardware-dependent,
  which the purity rule exists to prevent.
- **Track the next tick index across blocks.** Rejected: it would silently encode seek semantics, and
  the block's own frame is authoritative anyway.
- **Let the transport tap keep its entries (record rather than drain).** Rejected: the tap is a runtime
  feed, and the session log is already the durable record of `play`/`stop` — two records would drift.
- **Surface `overflows()` as a context service now.** Deferred to slice B, which is where a shell or the
  host would report it; adding a service with no consumer is speculative.
- **Emit ticks only when a sink is present.** Rejected: it would make the tick arithmetic untestable
  without a device and couple generation to transport of the bytes.

## Consequences

- The engine can now generate a correct MIDI-clock stream with no MIDI code in it at all: no
  dependency, no device, no ALSA/JACK/midir — the sink is a seam, and the seam is empty by default.
- Tested: 48 ticks per second at 120 bpm 48 kHz at frames 0, 1000 … 47000 asserted to the sample; a
  tempo change inside a block stays exact through `frame_at`; transport lands at logged frames with a
  late feed at offset 0; overflow is counted, never grown; a session with neither service mounts and
  renders; and the existing counting-allocator test now mounts `clock_out` and still measures zero
  allocations on the render path.
- **Two fixture-only edits in `crates/media`** (`arranger.rs`, `stream.rs` test modules) were needed
  because `NodeIO` gained a field; no library code there changed. Slice B's boundary is otherwise intact.
- **Honest limits.** The claim is *scheduling* accuracy against the tempo map, not wire jitter — MIDI
  clock has no frame domain, and the real sink's timing is slice B's problem. The transport tap is
  write-and-drain, so nothing but the session log can reconstruct transport history. `overflows()` has
  no consumer yet. And tick 0 coincides with beat 0, so a `Start` at frame 0 shares a block with a
  clock tick; whether real gear tolerates status and clock in the same instant is a hardware question,
  recorded here rather than guessed.

*Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-29.*
