# Agent Note: Trigger identity — pulses carry a tag, not a bare offset

Status: proposed

## Problem

The decoupled pitch/rhythm note wants Polypulse's `note sel = pulse 1` mechanic: one pulse echoes the note another pulse most recently played. That is impossible against today's signal vocabulary. `Trigger` is a bare type alias for the sample offset (`pub type Trigger = u32`, `graph.rs:38`) — a timestamp with no notion of *which* pulse fired. When two rhythms fan into one note stream, the events are indistinguishable by origin, so `follow_pulse` (and any per-pulse post-processing like per-pulse swing or octave) has nothing to key on. The trigger/note format hardens across Phase 1 into Phase 2; adding identity after the format is widely used is invasive.

## Proposal

Promote `Trigger` from a bare `u32` to a small struct carrying an **origin identity** alongside the sample offset, and thread that identity through to `NoteEvent`:

- `Trigger { offset: u32, pulse: PulseId }`, where `PulseId` is a cheap, comparable id (a `u16` or a small newtype over a `usize` index into the producing node's pulse lanes).
- `NoteEvent { offset, pitch, velocity, duration, pulse }` gains the same `pulse` field so the assignment policy (`follow_pulse`) and any downstream per-pulse logic can see origin even after the note is generated.
- `EuclideanGen` and future interval/offset/reset pulse generators **tag each emitted trigger** with the lane it came from. Fan-in (`insert_sorted`) preserves the tag; the sort key remains `offset` (the tag breaks ties deterministically, so merged ordering stays total and replays identically).

The tag is a *provenance* hint, never a routing decision: nodes still consume by port, and the patch bay's type-checking is unaffected (the `Trigger`/`Note` kinds keep their name; only the *payload* grows a field). Nothing in Phase 1 reads `pulse`; it is additive, with the field defaulting to a "unattributed" sentinel (e.g. `PulseId(0)` = unknown) so MIDI/OSC-sourced triggers and existing tests remain valid.

## Alternatives considered

- **Keep `Trigger = u32`; identify pulses by ordering/position** — brittle: it assumes a fixed pulse→position bijection that rotation, offset, probability, and polyrhythm all break, and it can't survive a `random`/markov reorder. Rejected.
- **Encode identity in a separate parallel stream** (a second `Trigger`-typed edge carrying ids) — splits one logical event across two synchronized buffers, doubling fan-in bookkeeping and making drop/reorder divergence possible. Rejected.
- **Give each pulse its own port** (`out("trig0")`, `out("trig1")`, …) — explicit but kills the "many pulses, one rhythm object" ergonomic, and forces the assignment node to fan-in N ports manually. Rejected: the tag is the same information with one port.
- **Defer identity until a plugin needs it** — tempting, but the note's own risk text says the format hardens; retrofitting `Trigger` to a struct touches every `EventBuf<Trigger>` sink and every test after the fact. Rejected.

## Acceptance criteria

- `Trigger` is a struct with `pulse: PulseId`; `EuclideanGen` tags its triggers by lane.
- `NoteEvent` carries `pulse`; the `scale`/`assign` path preserves origin through to the note.
- Fan-in merge stays sorted by `offset` with deterministic tag tie-break; replay byte-identical.
- A `follow_pulse(pulse 1)` assignment mode is expressible end-to-end and tested (pulse 2 emits the pitch pulse 1 last played).
- Unattributed triggers (MIDI/OSC, sentinel id) behave as today; existing Phase-1 tests pass unchanged.

## Risks

- **Payload size / event-buffer capacity** — adding a field grows `EventBuf<Trigger, CAP_EVENTS>` and the merged buffers; verify `CAP_EVENTS`/`MERGE_CAP` still hold the worst-case Phase-2 fan-in with no recompile surprises on the no-alloc render path.
- **Sentinel id collisions** — if `PulseId(0)` means both "lane 0" and "unknown," a real lane-0 pulse and an unattributed trigger become indistinguishable. Mitigate: reserve a distinct `NONE` sentinel (e.g. `u16::MAX`) rather than reusing lane 0.
- **`follow_pulse` reads a mutable "last note" of another lane** — that is shared cross-lane state on the assignment node, touching the same determinism seam as `random`/`markov` (see the [PRNG determinism note](2026-08-20-determinism-random-markov-log.md)); the "last note" map must be logged/checkpointed for replay, not held in unchecked node memory.
