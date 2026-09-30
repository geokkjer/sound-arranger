# Agent Note: A crossfade too short to mix in is a cut, not a fade

Status: implemented

## Problem

A splice's `crossfade` is a `u32` off the `host v1` text form, whose operand has no
minimum — and `0` is the natural spelling of "no crossfade" (`splice 48000 take.wav 0`).
The render path clamped it to one sample and then ran the equal-power form over that
one-sample window:

```rust
let denom = (fade.total - 1).max(1) as f32;  // total 1 → 0 clamped to 1
let t = pos as f32 / denom;                   // pos = 0 → t = 0
let g_in = (std::f32::consts::FRAC_PI_2 * t).sin();  // sin(0) = 0
```

`pos` is 0 on the **first** window sample at any length, so `g_in` was 0 and the
incoming clip's first sample was multiplied by zero and discarded. A caller who asked
for a hard cut got a cut one sample late with a sample of the new clip deleted — audible
on a transient or a downbeat. The comment above it ("the last fade sample is pure
incoming, no gain step at the end") was false at `total == 1`: the only window sample was
pure *outgoing*.

At the edge it was worse than a lost sample. An incoming clip of **one frame** has nothing
but that first sample: it was consumed by the window, multiplied by zero, the ring then ran
dry with `eof` set, and every later `pop_sample` returned legitimate silence — the clip was
inaudible for its entire life, and nothing counted a fault.

The magnitude is one sample, 20.8 µs at 48 kHz. It is worth fixing because the value is
reachable from the declared wire schema, because `0` is what a user writes for a cut, and
because of the one-frame clip. Found by the Space Bunny review (claim `2-media-io#2`),
confirmed independently, and deferred once as "small and real"; it is now fixed.

## Decision

**A window too short to mix in — `0`, or the `1` it is read as — is a hard cut**, applied
sample-accurately at `at_frame`.

- `PlaybackNode::render` branches on the window inside the existing `Fade`: at
  `fade.total <= 1` the sample is the incoming clip's next sample at gain 1, and the
  outgoing is not popped at that frame (a cut consumes nothing from the clip it leaves).
  Any longer window takes the equal-power path unchanged.
- The cut therefore lands on the requested frame even when the command is due **mid-block**
  — the pre-fade `offset` path is untouched — and the incoming clip runs from its
  **first** sample, with the outgoing's last sample the frame before. That is what the
  equal-power form cannot express in one sample, and it is the fix.
- `SpliceCmd::crossfade`'s doc states that a too-short window is a hard cut, which is also
  the contract the `host v1` form round-trips (`format_command` writes back whatever it
  holds, `media_ops` decodes it verbatim).
- `cmd.crossfade.max(1)` stays, and now only to keep `remaining` from underflowing: the
  degenerate window *is* the cut, so the clamp no longer decides behaviour.

**A window of 2 or more samples is unchanged.** `pos = total - remaining` still runs
0…N−1 over `denom = total − 1`, so the last window sample is still pure incoming — with
the incoming's first sample consumed at `g_in = 0`, which is what an equal-power fade
means, and which `splice_during_playback_is_sample_accurate` pins by asserting the
post-fade output equals B from index `xf`. (The review's claim that nothing is lost for
`total ≥ 2` is wrong in the sense that matters; this note changes none of that arithmetic.)

Two regression tests beside the other `stream` unit tests:

- `a_crossfade_too_short_to_mix_is_a_sample_accurate_cut` — two position-revealing ramps
  that share no value at the seam (B's ramp is phase-shifted, so its first sample is not
  `0.0` and cannot pass for silence), spliced at a frame mid-block, for `crossfade` 0 and
  1: pure A before, the incoming's **first** sample at the cut frame, then B from its
  second sample on, unshifted, with zero underruns and zero deferred.
- `a_one_frame_incoming_clip_is_audible_through_a_cut` — a one-frame incoming clip, whose
  single sample is `0.75`: that sample is emitted, and the silence after it is legitimate
  (clip over), not an underrun.

Both fail on the pre-fix logic — the cut frame carried A's sample (`0.891` where B's first
is `0.398`), and the one-frame clip's `0.75` was replaced by A's.

## Alternatives considered

- **Clamp the window to two samples and document that a 1-sample crossfade is a 2-sample
  window** (the review's second suggestion). Rejected: it does not fix the reported
  defect. `pos` is 0 on the first window sample at any length, so the incoming's first
  sample is *still* multiplied by zero; all it changes is which sample is pure incoming.
  It also invents a window length the caller did not ask for.
- **Branch on `crossfade == 0` at command application and set
  `self.cur = Some(Player::new(cmd.incoming))` directly** (the review's preferred shape).
  Rejected: it drops the window, and with it the `offset` the command carries, so a splice
  due mid-block would apply at the block start — the node's whole reason for existing is
  that the cut lands on the frame, and this fix is *about* landing on the frame. Keeping
  the cut inside `Fade` gives the same intent with the offset intact and one code path for
  an on-time and a deferred command.
- **Refuse `0` in the host's parser (`Err("crossfade must be at least 1")`).** Rejected:
  `splice <frame> <clip> 0` is exactly how a user spells a cut, so refusing it breaks the
  script people will write to get the behaviour this note implements. The
  [fade-sums note](2026-09-29-fade-sums-are-checked-not-wrapped.md) refuses an
  unrepresentable pair, and its reasoning is why this is not that case: there is no
  minimum worth enforcing, because 0 is now meaningful rather than degenerate.
- **Treat a 1-sample crossfade as a real one-sample equal-power mix (`t = 0.5`, both gains
  0.707).** Rejected: a window one sample long has no ramp to spend — the midpoint is a
  value with no time axis — and it would make `1` behave unlike `0` for no gain. Cut is
  the honest reading of "too short to mix".
- **Fix the unbounded mailbox drain in the same change** (the `VecDeque::with_capacity(8)`
  comment's documented Phase-0 edge). Deliberately not bundled: a different defect with
  its own decision, already recorded where it lives.

## Consequences

- `splice <frame> <clip> 0` is a hard cut at `<frame>`, and `1` is the same cut. A
  one-frame incoming clip is audible. The
  [Spike B note](../architecture/2026-08-17-spike-b-media-engine.md)'s crossfade contract is
  now complete at the degenerate end, which is where its `denom.max(1)` guard was papering
  over an undefined result rather than a legal one.
- **No change for a legal crossfade.** `splice_during_playback_is_sample_accurate`,
  `bounce_is_byte_identical_on_replay` (`crates/media/tests/spike_b.rs`) and the three
  512-sample splice assertions in `crates/host/tests/reference_host.rs` pass unchanged.
- The render path stays bounded, allocation-free and panic-free: one branch on a `u32`
  already in hand, no new state, no new pop. `denom`'s `.max(1)` is gone because the
  division is now only reached where `total >= 2`, so it cannot be degenerate.
- The wire form needed no change, and `0` round-trips through save/load as before.
- Not touched: `docs/architecture-explainer.md` §6.3 describes the equal-power form for a
  real window, which is still exactly what it says; it never mentioned the degenerate
  case, and touching it would mean advancing its freshness banner against a commit that
  does not exist yet.
- Tests: `cargo test -p media --lib` green (125 passed, 3 pre-existing ignores);
  `cargo test -p media --test spike_b` green (7 passed, 2 hardware/soak ignored).

*Authored with Space Bunny · OpenCode, 2026-09-29.*