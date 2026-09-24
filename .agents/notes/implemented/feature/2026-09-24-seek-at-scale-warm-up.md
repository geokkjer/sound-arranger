# Agent Note: seek at scale — the warm-up seek (and why not a held session)

Status: implemented

## Problem

The plan's item 15: "**Seek at scale**: checkpoints (a rebuilt session held at intervals, provably
equal to a replay) so a jump into a 30-minute arrangement is interactive."

Measured first, because the answer changes everything. A backward seek is a rebuild plus a render of
the timeline from frame 0 (`replay_to` → `rebuild` → `render_to`), so its cost is the whole piece up
to the target. On a 30-minute, four-track arrangement with a master chain, in release:

```
$ time host seek30.txt          # transport seek 86400000 (30 min)
real  0m10.6s                   # a ten-second freeze on every backward jump
```

Ten seconds is not interactive, so item 15 has a real problem to solve — but *not* the one the plan's
parenthetical assumes.

## Decision

**A warm-up seek: place the clock just before the target and render a short run-in.**

- `Clock::seek_to(frame)` (and `Engine::seek`) place the clock **without rendering** what is skipped.
- The host's `SEEK_WARMUP_FRAMES` (48 000 = **1 second**) is the run-in: after placing the clock at
  `target - 1 s`, one second of timeline is rendered, which is all a stateful bus effect needs to
  reach the state a full replay would have had there.
- **Readers need nothing.** `ArrangerNode::render` derives every read from `block.frame` (its clips
  are a pure function of the frame), so a reader mounted at an offset reads exactly what the
  sequential path would have read — the run-in exists only for effects (the master's compressor
  envelope and its 5 ms lookahead delay line).
- **Soundness is guarded, not assumed.** `can_warm_seek` refuses the short cut when any state is
  placed *after* the run-in's start (a compressor mounted mid-piece, a parameter change inside the
  run-in, an edit stamped after the target): such state has to be applied at its own frame, which
  only a full replay can place. The legacy recorder-player path (`play`/`splice`, which streams with
  its own buffering) is excluded too. A session that is mid-recording is refused by the same rule
  that already stops the recording first.
- **The proof is byte-equality, not an argument.** `a_warm_seek_equals_a_replay` renders the same
  window from a warmed seek and from a full replay and compares the **audio bytes** for a session
  whose master chain is live across the seek (clips every second for two minutes, so the limiter's
  state at the target is real work), plus the arrangement value and the final clock. A second test
  proves the same for a broken session whose clip runs past its source's end.
- **The result is reported**: `HostSession::last_seek()` → `(frame, warmed)`, on the outcome and in
  the CLI summary, so a shell can say *why* a jump was fast.

Measured on the same fixture after the change:

```
$ time host seek30.txt          # warm-up
real  0m0.097s                  # ~110× faster, and it renders the same bytes
```

## Alternatives considered

- **A held rebuilt session at intervals** (the plan's parenthetical: "a rebuilt session held at
  intervals, provably equal to a replay"). Implemented, tested — and then removed, because it cannot
  pay for itself. A held session is **single-use** (a resume adopts it; `HostSession` holds opaque
  `AudioNode`s and reader threads and cannot be cloned), so *creating* one costs exactly the render it
  saves, and consuming it removes the benefit for the next seek. Measured and reasoned: the held-
  session design made the first seek neither faster nor slower and the second no better. It is
  recorded here as the rejected path rather than shipped as a non-feature.
- **A cloneable session** (snapshot every node's state so held checkpoints could be reused). Rejected
  for alpha: it needs a snapshot/restore contract on `AudioNode`, which every plugin would have to
  implement — a large architectural change for an optimisation, when the run-in reaches the same
  state in one second.
- **Holding rendered audio** (cache the mixed timeline in chunks and splice it in). Rejected: the
  graph state at the seek target is what playback needs (readers, effect state), not a buffer of
  samples; and it would double the memory of a long piece.
- **A longer or shorter run-in.** One second is bounded by the profile, not by taste: the compressor's
  attack/release (~0.25 s) and the limiter's 5 ms lookahead are far inside it. A future effect with
  longer memory (a reverb, a long delay) must raise the constant — and the equality test is what
  fails if it is too short, which is the point of expressing the bound as a constant with a proof.
- **Keeping the full render for sessions with late state** (no warm-up at all). That *is* the
  fallback; the guard is what makes the optimisation safe rather than approximate.
- **Rendering backwards** from the current position (negative reads). Rejected: the graph and the
  clock are forward-only by design ("the core clock only advances by rendering"); a backward render
  would need every node to be reversible.

## Consequences

- **A jump into a 30-minute arrangement is interactive**: ~0.1 s instead of ~10.6 s on the measured
  fixture, and the bytes are provably the same ones a full replay would have rendered.
- **The optimisation is invisible when it cannot be proved**: a session with late-placed state (or the
  legacy player path) takes the old full-replay route, and the shell's summary says which ran.
- **`Clock::seek_to` is a new sharp edge in the engine.** It is documented as "the caller owes the
  graph a warm-up": readers are frame-pure, effects are not. The host is the only caller, and its two
  tests are the contract.
- **A reader mounted past its source's end now plays silence** instead of failing, and the bound is
  correct in two ways the gate had to prove: it is measured from **`src_start`** (the reader mounts at
  `src_start + phase`, so the material left is `src_frames - src_start`) and it **never touches a
  looped clip** (whose phase is `off0 % loop_len` and whose cycle is `off0 / loop_len` — bounding it
  moved both, which the gate's probe caught as a wrong-cycle read in release and an
  alignment-invariant panic in debug). A clip whose declared region exceeds its source is still a
  broken session; it now behaves the same way whichever path reaches it.
- **A mid-piece `SetTempo` is frame-placed value state**, so the warm prefix keeps it at its own frame
  (placing the clock there and scheduling the change) instead of moving it to 0: the audio would be
  right either way (the render reads frames) but every beat reading, the ruler and the position readout
  would disagree with a full replay. Getting that right exposed a **latent engine bug**: a tempo event
  delivered *late* was stamped at the clock's current frame rather than its own, so
  `SchedEvent::SetTempo` now carries `at_frame` and the segment lands where the change belongs. The
  equality is pinned by `a_warm_seek_keeps_mid_piece_tempo_placement` (beat equality to the bit).
- **Still open**: a *pre-warmed* cache at session load (paying the run-in eagerly for the piece's
  common landing zones is measurable but not obviously worth the load-time cost); stating the bound
  as a per-node declaration (`AudioNode::memory_frames()`) instead of a constant, so a reverb can
  raise it automatically; and the pool/stream path (`play`/`splice`) coming under the same guarantee.

## The gate's findings

The slice's gate returned `merge with changes` ([review +
disposition](../../../../research/architecture/2026-09-24-alpha-slice-gate-g2-glm-standin.md)); its
findings were real and are fixed, with tests:

1. **The EOF clamp broke a loop's phase** (must-fix) — the reviewer's probe reproduced a debug panic on
   the alignment invariant and, in release, a warm seek playing the wrong cycle. Fixed: the bound
   applies only to a contiguous read.
2. **The clamp ignored `src_start`** (must-fix) — a clip with `src_start > 0` and an over-declared
   region still failed `seek_frames past end`. Fixed to bound by `src_frames - src_start`.
3. **`last_seek` was set by undo/redo** (should-fix) — an edit announced itself as a seek. Fixed with
   an `is_seek` flag; an edit carries the last real seek's report across instead of blanking it.
4. Recorded: a hardcoded 48 000 in the CLI's run-in print (fixed to the session rate), a pre-existing
   backdated-`set_tempo` live/replay divergence, and the flaky capture test.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-24.
