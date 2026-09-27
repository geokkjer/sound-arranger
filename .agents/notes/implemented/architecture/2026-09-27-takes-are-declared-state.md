# Agent Note: Takes are declared state — the capture is the side effect

Status: implemented

## Problem

`record <take_id>` and `record stop` were **pure actions**: absent from `is_state()`, and so from
`session.txt` and the journal. A saved session kept the take's WAVs in `pool/` but nothing in the
document said a take existed, where it started, or how long it ran — the pool listing could show
files, and the log could not say what they were.

Logging the `record` line instead is impossible by construction: replaying it opens a device. That
is precisely why it was an action, and it is the reason the recorder needed a different fact to
keep — not the act, but the result. This is the first slice of the recorder work in the
[profiles note](../../proposed/architecture/2026-09-27-profiles-and-the-umbrella-name.md)
(criterion 4), and it takes the shape the
[capture-topology note](../../proposed/architecture/2026-09-25-capture-topology-aligned-stems.md)
already gives sources: *a logged declaration, not UI state*.

## Decision

**The capture is a side effect; the declaration is state.**

- `HostCommand::Take { take_id, frames, dropped, channels, at_frame }` is state. `record stop`
  commits it through `commit_state` — extracted from `process` and now shared, so the declaration
  reaches the history and the **journal** by the same path as every other state command. A crash
  mid-session therefore keeps the take, not only the WAV.
- **Replay binds it without a device and without reading a file.** The pool ids are derived from
  the `{take_id}.ch{k}` convention every clip already references, and the rate is the session's.
- **`dropped` is carried** because the WAV cannot record its own gaps: a take that lost frames to a
  full ring is missing them silently, so the declaration is the only place that fact survives.
- **`at_frame` is the take's origin**, and `Take` deliberately holds a plain `u64` rather than the
  `Option<u64>` an edit carries: `at_frame()` returns `None` for it, so a take is never
  pre-rendered or scheduled like an edit. It is where the capture started, not when a command ran.
- **The purity boundary is the pool file.** The capture sits outside it — device-bound, not
  reproducible; from the file onward the render is byte-identical, which is the invariant the
  recorder is built to prove.

## What this slice deliberately does not do

No **placement** (the take is not on the timeline yet — the next slice), no alignment or offset, no
runtime take **list** on `HostSession` (`last_take` stays the shell's indicator; the log and the
pool listing are the record, and a list earns its place when placement needs one), and **no device
on the take**: the input handle carries no name today, and device identity belongs to the rig's
`source add` declaration per the capture note rather than to every take.

## Alternatives considered

- **Log the `record` line as state.** Rejected: replay would open a device — the reason it was an
  action to begin with — and a document that cannot be replayed is not a document.
- **Carry the declaration through the `Event::Arrangement` carrier as a `MediaTake` op.** Rejected:
  the engine never needs a take to render (clips reference pool sources), so the op would be
  machinery with no consumer, which the budget rule forbids. `MediaPool`/`MediaPlay` earn their ops
  because the graph reconciles from them; a take earns none.
- **Record it in the engine log beside `MediaBounce`/`MediaExport`.** Rejected: those are *action
  outcomes* kept for observability; a take is state, and `session.txt` is the state document.
- **List the pool source ids explicitly in the line.** Rejected: they are `{take_id}.ch{k}` by
  construction — the capture builds them that way and every clip references them — so the line would
  restate a convention it could then disagree with. The declaration names the take and its shape.
- **Record a per-take `sample_rate`.** Rejected: takes are written at the session rate by
  construction (the device's clock is drift-compensated), so a per-take rate would be redundant data
  that can disagree with `session_rate`.
- **Keep a runtime take list now.** Deferred: no consumer until placement exists.

## Consequences

- A session directory describes its own material, and the journal makes a finished take crash-safe.
- `TakeReport` and `Recording` gained `at_frame`; on replay `TakeReport.sources` is derived exactly
  as the capture derives it, so a loaded take and a live take are equal values.
- The text form is `take <take_id> <frames> <dropped> <channels> <at_frame>`, an additive
  extension of `host v1`. `save` refuses any state command with no text form, so the two lists in
  the round-trip test are the format's contract: `Take` joined the state list, and `Record` stays
  in the action list.
- Two tests: a finished take is committed to the document **and** the `record` line is not, and a
  `take` line replays with no device and no pool.
- The next slice (a capture produces a placement) builds directly on `at_frame`.
