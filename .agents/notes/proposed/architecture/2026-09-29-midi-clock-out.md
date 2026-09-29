# Agent Note: MIDI clock out — sending time without owning it

Status: proposed

> **Partly shipped (2026-09-29):** the engine half — the currency, the `clock_out` plugin, the tick
> math and the purity test — is [implemented here](../../implemented/architecture/2026-09-29-midi-clock-out-in-the-engine.md).
> The host wiring, a real `media` sink and the `clock=follower` binding are slice B and still open.

## Problem

The recorder profile rests on external gear following the session's clock (or the session following
theirs). The rig now declares each source's `clock` role — `master`, `follower`, `free` — and the log
carries that declaration, so the missing half is the one that actually *sends*: a `follower` is a
destination we must drive with MIDI clock (24 PPQN) and transport (`Start` / `Stop` / `Continue`).

What exists: `MidiSink: EventSink` as a declared seam with **zero implementations**, and no MIDI
backend anywhere in the workspace. `ExternalEvent` carries notes, triggers and control — but no clock
and no transport, so the currency for realtime messages does not exist yet either.

Three properties make this more than writing bytes to a port:

- **The clock is the core's, read-only** ([the minimal-core note](2026-08-15-minimal-core-clock-graph-session-log.md)):
  no plugin owns time. Clock-out therefore *reads* what the scheduler reads, from the same tempo map.
- **Sending is a device-bound side effect**, so a replayed session must reproduce the *declaration*
  and never the sending — the rule the takes slice follows for capture.
- **Ticks must be sample-accurate against the tempo map.** Sending from a wall clock drifts against
  the session by exactly the amount that makes later alignment impossible, which is the whole point of
  the profile.

## Proposal

1. **Currency.** Extend `ExternalEvent` with the realtime vocabulary:
   `Clock { offset }`, `Start { offset }`, `Stop { offset }`, `Continue { offset }` — `offset` being
   the sample within the block, matching the shape the existing variants use. The outbound direction
   already has a contract for "these events, at this frame" (`EventSink::send`), so this is the
   smallest change that keeps one event currency for both directions.
2. **The generator is a plugin** (`clock_out`), mounted like any other, `inject`ing nothing: for each
   block it computes the ticks whose frames fall inside that block from the tempo map, plus transport
   at the frames the logged transport commands take effect. It holds a `Box<dyn MidiSink>`, not a
   port.
3. **Device ownership stays in the host and `media`**, next to the audio device path: the host opens
   the MIDI output (chosen by the rig binding when that slice lands) and constructs the plugin with
   the sink. A plugin that opened its own port could not be tested without the device, and would put
   device lifecycle in the graph.
4. **Purity.** The mount and its declared destination are **state** — logged, replayed, visible in
   `session.txt`. The sink is opened by the host, never by a replay. A replay on a machine without the
   device sends nothing, says so, and renders **byte-identically**: whether the gear is attached cannot
   change the audio.
5. **Testability.** A fake sink records `(frame, event)` pairs, so tick *frames* are asserted against
   the tempo map — 24 PPQN at 120 bpm 48 kHz is one tick every 1000 frames exactly — including across
   a tempo change, across a rebuild, and with a block boundary falling between ticks. A real-device
   test is `#[ignore]`d, like the capture ones.
6. **Scope.** 24 PPQN, `Start`, `Stop` and `Continue`. Song Position Pointer, tempo re-sync
   mid-flight, and MIDI note/CC output are explicitly **out of this slice**.
7. **Destinations.** The first slice mounts with an explicit port; binding a `clock=follower` source's
   `matcher` to a real port is the registry/binding slice, which is what the rig declaration was for.

## Alternatives considered

- **A MIDI-specific sink API** (`fn send_clock(&mut self, …)`). Rejected: it forks the outbound
  currency in two, and the existing sink contract already means "these events, at this frame".
- **Generating the clock in the host instead of the graph.** Rejected: `RenderBlock` gives a node the
  block's frame, so the graph is where sample-accurate scheduling already lives; the host has no
  render hook to hang it on.
- **Sending from a wall clock in the device thread.** Rejected: it drifts against the session, which
  is the one thing this profile cannot tolerate. The tick *schedule* comes from the tempo map; only
  the wire timing is the device's business.
- **One `Scheduler<SchedEvent>` event per tick.** Rejected as the mechanism: it would put 48 events
  per second through the block-quantised scheduler for arithmetic the plugin can do directly from the
  block's frame range.
- **The plugin opening its own port.** Rejected: untestable without hardware, and it puts device
  lifecycle inside the graph.
- **Implementing Song Position Pointer now.** Deferred: SPP needs "what is a bar" in the *gear's*
  terms, which is a decision about musical position, not about sending.
- **Writing straight to the device from the render path.** Rejected outright: a blocking write in the
  render path is a realtime violation. The sink is fed from a bounded queue drained off the render
  thread — the same shape capture uses for its ring.

## Acceptance criteria

1. With a fake sink, N seconds of playback at a known tempo produce exactly
   `N × 24 × bpm / 60` ticks, at frames matching the tempo map to the sample, including a tempo change
   inside a block.
2. Transport follows the logged commands: `play` → `Start` (or `Continue` after a pause), `stop` →
   `Stop`, at the frame each command takes effect.
3. Rendering is **byte-identical with and without a sink**: the device cannot influence the audio.
4. A session that mounts clock-out replays with no device: the declaration is in `session.txt`, the
   replay sends nothing, and it reports that rather than failing.
5. The render path allocates nothing (the counting-allocator test still passes) and never blocks.
6. `cargo test --workspace` and CI stay green; the hardware path is an `#[ignore]`d test.

## Risks

- **What accuracy we can claim.** MIDI clock has no frame domain: ticks are sent when due, and the
  wire's jitter belongs to the device and the OS scheduler. The claim this slice can make is
  **scheduling** accuracy against the tempo map — not wire jitter. Saying otherwise in the docs would
  be the kind of claim this project keeps refusing to make.
- **A blocking device write** is the main realtime hazard, which is why the queue is in the design
  rather than discovered later. Its depth is a bound that has to fail loudly, not grow.
- **Seek and rebuild semantics: decided, conservatively (2026-09-29).** A seek rebuilds the session,
  so gear mid-pattern would jump. The decision is that **nothing is sent across a rebuild**, and the
  next `play` re-syncs (`Start`, or `Continue` after a pause) at that frame. Rejected:
  `Stop` … `Continue` around the rebuild (more correct in principle, and it assumes the gear tolerates
  the pair), and refusing clock-out while seeking (a refusal the player cannot act on). This is
  expected to be revisited once real gear is on the wire — bugs here will show themselves in use,
  which is the honest reason to start with the option that cannot corrupt anything.
- **No binding yet.** The first slice takes an explicit port; the rig's `matcher` cannot resolve to
  one until the registry slice lands.
- **Gear-specific behaviour** (what a particular device expects on the wire) belongs to the studio
  project, linked rather than copied, per the separation rules.

*Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-29.*
