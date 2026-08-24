# Agent Note: Decoupled pitch & rhythm — shared note lists, pulse mechanics, and a selection-policy combinator

Status: proposed

## Problem

Polypulse (Lambda Synthetics) validates a generative-music idea we already hold in Phase 2: rhythm and pitch compose independently, and the expressive payoff comes from *swapping one against the other* — turning one knob to change the rhythm while the pitch object stays fixed, or vice versa, "discovering melodies you hadn't thought of yourself." Our patch bay already decouples them at the *module* level (the `euclidean → scale → tone` chain is data, not code), but not at the *semantic* level: `ScaleGen` is a single `forward` counter over a privately-owned degree vector, so pitch objects cannot be named, shared, or swapped, and there is no place for a selection policy to live. The target is Polypulse's *mechanics* — interval/offset/reset pulses with per-pulse note selection — not just its abstract idea. A "chords" companion is deferred and out of scope here; this note is about the rhythm/pitch decoupling and its seams.

## Proposal

Split the note pipeline into three independently-addressable concerns, joined by a cross-product that is itself patchable:

1. **Rhythm objects** (the "pulse" side) — `euclidean` today; interval/offset/reset pulse generators later. Each emits `Trigger` events carrying an **identity tag** (`pulse: usize`), not a bare timestamp.
2. **Note lists** (the "pitch" side) — promote `Scale`'s `degrees` and `root` into a **named, shared service** (`ctx.note_list`, alongside the existing `rhythm` and planned `progression`). A note list is `{ id, root, degrees, expressions }`, provided once, consumed by many. Multiple rhythms reference the same list — different rhythms, similar pitches.
3. **The assignment policy** — a small combinator plugin `in("trigger") → out("note")`, parametrized by `mode ∈ { forward, reverse, fwd_rev, random, markov, follow_pulse }`. *This* node owns the counter and any Markov/PRNG state, and reads the shared note list. Polypulse's `note sel` is exactly this node, made first-class and patchable.

Chain becomes:

```
euclidean(pulse) ──► assign(mode) ──► tone ──► audio
                        ▲
                 note_list (shared, named)
```

Swapping the note list or the rhythm is one patch; neither touches the other. The paper's "improv" component (Phase 2) becomes the *explorer* of this cross-product: a plugin that repatches rhythm→assign and note_list→assign and logs each patch, so the machine performs the combinatorial search with the session log as memory. The decoupling is therefore not a UX nicety but the substrate that makes an automated composer addressable.

### Two seams that must be decided before Phase 2 seals a format

Both seams are now their own decision notes — flagged here, decided there:

- **Trigger identity** — `follow_pulse` (a pulse echoing another's most-recent note) requires knowing *which* pulse fired; `Trigger` is currently an anonymous `u32` offset. See the [trigger-identity note](2026-08-20-trigger-identity-pulse-tag.md).
- **Determinism vs. the log** — `random` and `markov` hold hidden PRNG state, but the core's invariant is byte-identical replay. See the [PRNG determinism note](2026-08-20-determinism-random-markov-log.md).

## Scope clarification: the "no MIDI" lock is a *deferral*, not a ban

`RESEARCH.md` §1 reads "MIDI enters only later as control, never as a note-sequencing UI." That language has been misread as a philosophical rejection of MIDI the *format*. The intent is narrower: **defer MIDI note sequencing until the audio mixer/arranger is up and running**; MIDI note events are a Phase-2+ `MidiSource`/`MidiSink` integration, not a forbidden substrate. The musical-event model note already distinguishes the two ("no MIDI for the *sequencing UI*; generators produce note events as values"). A note list is a *pool + policy*, not a *sequence* — keep it that: the closer a note list drifts toward an ordered row of specific pitches, the closer it drifts toward being a piano roll, which must stay out of the timeline UI. Polypulse was a good jumping-off point for this clarity; do not over-index on it beyond that.

## Alternatives considered

- **Copy Polypulse's per-pulse `note sel` enum onto `ScaleGen`** — welds the policy to a single merged trigger stream, so it cannot share a note list across rhythms or express "this rhythm walks backward over the same list that rhythm walks forward." Rejected.
- **Share only `degrees` via service, keep `forward`-only** — captures the sharing but misses the generative engine (`markov`, `random`, `reverse`, `follow_pulse`), which is the actual source of variety. Rejected: the mechanics are the point.
- **Full VCV-style per-sample CV matrix** — already deferred in the patch-bay note; overkill for typed trigger/note streams. Rejected again here.
- **Make the note list a pair `(pitch, expressions)` carried on the note event** — puts pitch back on the trigger, re-welding the two concerns. Rejected: breaks the decoupling that is the whole point.

## Acceptance criteria

- `Trigger` carries a `pulse` identity; `EuclideanGen` tags its pulses; `follow_pulse` is expressible.
- A `note_list` service exists (`ctx.note_list`, symmetric with `ctx.rhythm`/`ctx.progression`); two rhythm plugins can read the same list with no import between them.
- An `assign` combinator exposes the selection modes, owns the counter/PRNG state, and reads the note list by lookup.
- The PRNG-state-in-the-log decision is made and recorded before any `random`/`markov` mode ships; replay stays byte-identical.
- The "no MIDI = deferral" clarifier is marked in `RESEARCH.md` §1 so the lock is read as scheduling, not philosophy.

## Risks

- **Voice management / polyphony** (`RESEARCH.md` §14 risk 8) — many simultaneous pulses + a chord note list quickly exceed `ToneGen`'s `MAX_BLIPS`/`MAX_PENDING` and drop notes silently. The decoupling is musically inert until note→voice allocation/stealing is designed. This is the real project risk, not the pitch/rhythm split.
- **Determinism seam** (above) — a randomly-seeded Markov mode that slips past the log invariant would break the byte-identical-replay promise that the whole core rests on. Mitigate by making PRNG state a logged, checkpointed value.
- **Note list drifting into a sequencer** — the more list-editing resembles "ordered specific pitches," the closer to the deferred MIDI piano roll. Mitigate: model it strictly as *pool + policy*, never *order*.
