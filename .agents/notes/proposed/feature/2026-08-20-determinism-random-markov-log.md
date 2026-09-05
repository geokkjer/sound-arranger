# Agent Note: Determinism of `random` / `markov` — PRNG state is logged state

Status: proposed

## Problem

The decoupled pitch/rhythm note's selection modes include `random` and `markov` (semi-random note-picking over a Markov chain), and `follow_pulse` holds a per-lane "last note" map. Each of these is *hidden mutable state* inside the assignment node. That collides head-on with the core's founding invariant — the [patch-bay note](../../implemented/architecture/2026-08-17-patch-bay-typed-signal-streams.md) is emphatic: *"Patching is logged … replay reproduces byte-identical output."* An unmanaged PRNG that draws from wall-clock state or a hidden incrementing counter will render differently on replay, silently corrupting the one property the session log and the "improv" auditable-replay story both depend on. This must be settled before any stochastic selection mode ships — otherwise it arrives as a quiet trickle of non-reproducible sessions.

## Proposal

Treat the assignment node's stochastic **state as logged, checkpointed parameter state**, governed by the same rule as any other `SetParam`:

- The selection node owns a **deterministic PRNG** (an explicit stream — e.g. `ChaCha8` or `SplitMix64`/PCG — not the platform RNG), seeded from a logged value.
- On each stochastic event draw, the node advances the PRNG and **records the seed/step (or the advanced state) in the log** as a parameter event — no wall clock, no unseeded `rand::random()`, no hidden counter.
- Replay reconstructs the same state at the same frames: seeding identically at mount and consuming the logged draws reproduces the exact note sequence. An arbitrary-seed request is itself a logged event, so "random" becomes "random-but-recorded," and the default is a fixed seed for strict determinism unless the user explicitly requests otherwise.
- `follow_pulse`'s per-lane "last note" map is checkpointed the same way: it is part of the node's reconstructed state, not unchecked memory (see the [trigger-identity note](2026-08-20-trigger-identity-pulse-tag.md)).

The general rule, worth stating once and applied to *every* stochastic plugin (future windowing, granular, etc.): **any node that draws randomness (a) uses an owned deterministic PRNG, (b) logs enough state to reconstruct it, and (c) is byte-identical on replay.** No plugin is allowed a wall-clock or uncontroled-random fast path that trades reproducibility for convenience.

## Alternatives considered

- **Allow replay to diverge for stochastic plugins** — breaks the log's audit guarantee; the "improv" component's entire value is that the machine's search is *auditable*, i.e. re-runnable. Rejected.
- **Seed from wall clock and accept non-reproducibility, documenting it** — a footgun that would let a `random` note-list silently destroy the replay guarantee users rely on everywhere else. Rejected; not worth one exception when a logged seed is no harder.
- **Recompute Markov draws from the note list deterministically without a PRNG** (pure function of index) — loses the "semi-random, similar-to-source" generative quality that is the point of `markov`. Rejected: determinism and stochasticity are both wanted; the answer is *recording* the stochasticity, not removing it.
- **Persist the full draw history instead of seed/step** — correct but bloats the log and re-implements what a checkpointed seed already gives. Rejected: checkpointing is the minimal-state form of the same guarantee.

## Acceptance criteria

- A `random` mode and a `markov` mode each run deterministically: same seed + same inputs ⇒ byte-identical note output across replays.
- The log carries enough state (seed, and per-draw step or checkpoint) that an arbitrary-session replay reproduces the exact sequence without wall-clock reads.
- Arbitrary reseeding is a logged event; a fixed default seed keeps sessions reproducible without user intent.
- `follow_pulse`'s "last note" map reconstructs from logged state on replay.
- The rule is stated in the core's notes so future stochastic plugins (`granular`, `windowing`) inherit it rather than rediscovering the trap.

## Risks

- **Log growth** — logging one event per stochastic draw (rather than a coarse checkpoint) could balloon the log on dense markov arpeggiation. Mitigate: checkpoint at a coarser granularity (e.g. per block or per N draws) with the intervening sequence derived by deterministic re-advance, trading a little definition for log size; decide the granularity when the mode lands.
- **Thread-safety / no-alloc render path** — the PRNG must advance without allocation and without touching shared clock state on the audio callback; a small owned generator satisfies this, but it has to be verified against the counting-allocator tests that already guard the render path.
- **Scope creep into "all plugins"** — the rule generalizes naturally but enforcing it repo-wide is a later, separate decision; this note only binds the note/rhythm selection modes and establishes the pattern as precedent.
