# Agent Note: A geometry op leaves a clip the renderer accepts

Status: implemented

## Problem

The clip model's rule for fades is a **sum against the clip's length** —
`fade_in + fade_out <= src_len` — and two ops **shrink `src_len`** without
re-checking it. `RazorSplit` zeroed the two seam fades and kept the inherited ones on
the halves that did not own the seam; `ChopClip` copied the outer fades onto pieces that
are `src_len / times` frames long. Neither arm called `validate_clip`, so a split or a
chop could put a clip in the value that the model itself refuses.

That is not a value-model curiosity, because the renderer is the consumer:
`ArrangerNode::new` validates **every clip on a track** and refuses the *whole track*
over one of them, and `wire_arranger` propagates that refusal to `HostSession::render`.
So a legal, accepted and **logged** edit sequence made a track unplayable — nothing on
it plays, and every later edit fails with the same error until someone hand-writes a
`set_clip_fade`. Two keypresses reach it, both of which the shell already offers: `f` at
a clip's end sets a **full-length** fade-in (it caps at `src_len - fade_out`, so
`fade_in = src_len` is one press away), then `x` razor-splits at any earlier frame:

```text
arrange add_clip t0 c0 s1 0 48000 0 0 0 1.0     # a clip, no fades
arrange set_clip_fade t0 c0 48000 0             # the shell's `f` at the end: a full-length fade-in
arrange razor_split t0 c0 L R 12000              # -> L: src_len 12000, fade_in 48000
```

The verifier of the [fade-sum fix](2026-09-29-fade-sums-are-checked-not-wrapped.md)
found the same hole in a second field on the same arms. `src_start` is an unbounded
`u64` on the `add_clip` path and `validate_clip` never bounded it, while a **reversed**
split and a **reversed** chop both *add* to it (`c.src_start + right.src_len`,
`c.src_start + c.src_len`). `u64::MAX - 3_999 + 4_000` wraps: a debug panic inside
`ClipEditor::apply`'s timeline lock (the same poison the fade-sum fix removed), and in
release a silent wrap to a source offset the user never asked for. The *forward* arms
wrapped too (`c.src_start + split_in`) — the review named the reversed ones only.

## Decision

**An op that rewrites a clip's geometry leaves the clip satisfying
`validate_clip`, and no sum it computes can wrap.** Three changes in
`crates/media/src/timeline.rs`, all on control-side paths:

- **`RazorSplit`**: `left.fade_in` and `right.fade_out` are capped to the half they land
  on (`fade.min(src_len)`), both `src_start` sums are `checked_add`, and each half goes
  through `validate_clip` before the original clip is dropped.
- **`ChopClip`**: the first piece's `fade_in` and the last piece's `fade_out` are capped
  to the piece length the same way, the reversed seed `c.src_start + c.src_len` is
  `checked_add`, and every piece is validated.
- **`validate_clip`** gains the source-window bound `src_start + src_len` not
  overflowing, beside the existing `at_frame + src_len` span bound — the mirror of it,
  and the reason the two arms above cannot produce a wrapping sum from an accepted clip.

**`LoopRegion` also validates** (it is the one remaining growth arm, and it is what makes
the new bound maintainable). `checked_mul` keeps the *length* representable, but growing
`src_len` can still push `at_frame + src_len` or `src_start + src_len` out of range — a
clip placed near the end of the frame range, looped many times, was a third way to wedge
a track with a logged op. The review called this arm sound ("only grows `src_len`"); it
grows past an invariant the moment the clip sits near the end of the range.

**A shrink caps the fades; it does not refuse the op.** Capping is what `Stretch` has
always done when it repoints a clip at shorter material, what the shell's trim-to-content
gesture does (`refusing instead would make T unusable`), and what `ChopClip`'s own
comment intends by preserving the outer fades. A refusal would make `x` a no-op on any
clip carrying a long fade, which is the state `f` puts it in one press earlier. The cap
is not the parse-time coercion the [fade-sum note](2026-09-29-fade-sums-are-checked-not-wrapped.md)
rejected: nothing the user *typed* is rewritten here. The split itself already rewrites
the fades (it zeroes the seam on both halves), and a crossfade is a later `SetClipFade`.

## Alternatives considered

- **Refuse the split/chop and name the fade** (the review's second option). Rejected: it
  leaves the user unable to cut a clip at all while a fade is long, and the shell's own
  trim-to-content gesture set the precedent the other way — cap, and the cap is audible
  on the half that keeps the fade.
- **Cap the fades in the `host v1` parser instead.** Rejected for the reason the
  fade-sum note gives: `set_clip_fade` has no `src_len` at parse time, and a cap there
  rewrites a value the user typed. The cap belongs where the *geometry* changes.
- **Leave the fades and let `ArrangerNode::new` refuse.** Rejected: that is the bug. An op
  the log accepts must not leave the renderer unable to build a track.
- **Validate only the pieces, without the `checked_add`s.** Rejected: validation runs
  *after* the arithmetic, so a wrap would still panic in debug under the editor's lock.
  `Timeline` is `Deserialize`, so a value read from a snapshot is a legal `&self` to
  `apply` — a clip can reach an arm without passing `AddClip`.
- **Skip `LoopRegion`.** Rejected: the report's "only grows `src_len`, so it is sound" is
  true only while the clip sits well inside the frame range, and the new bound would
  otherwise be one `loop_region` away from being breakable by a logged op.
- **A `validate_after_rewrite` helper both arms call.** Deferred, like the
  `validate_fades` helper in the fade-sum note: two arms, one file, and the arms also
  need the cap and the checked arithmetic, so the shared part is one line.

## Consequences

- `f`-then-`x` on a full-length fade now splits, and the left half carries a fade-in
  capped to its own length. The seam stays hard (`left.fade_out = 0`,
  `right.fade_in = 0`), exactly as before.
- `add_clip` with a source window that cannot be represented (`src_start + src_len`
  leaving the range) is refused by name: `"clip 'c1' source window overflows"`. A clip
  whose window ends on the last frame is still legal.
- A `loop_region` that would push a clip's span or window out of range is refused
  (`"loop region: clip 'c0' span overflows the timeline"`). No behaviour change for a loop
  that fits.
- **No behaviour change for a fade that fits what it lands on**: a fade no longer than the
  half (or the piece) rides the split or chop untouched, and the interior chop seams are
  still hard.
- Five regression tests. Three new value-model tests, each verified to fail on the
  unfixed value: `a_razor_split_of_a_long_fade_leaves_two_valid_halves`,
  `a_chop_of_a_long_fade_leaves_pieces_the_model_accepts` and
  `a_source_window_past_the_frame_range_is_refused_not_wrapped` (the last reproduces
  `attempt to add with overflow` in the reversed arm). One assertion added to
  `loop_region_bakes_repeats_with_wrap_len` for the growth arm. And
  `a_razor_split_of_a_full_length_fade_still_bounces_audio` in
  `crates/host/tests/arranger_commands.rs`, which **plays** the two-keypress sequence
  end to end: `bounce` goes through `wire_arranger`, so the test fails on the unfixed
  value with the real `arranger: clip 'cL' fades exceed the clip length`.
- The render path is untouched: `fade_gain` and `ArrangerNode::new` are as they were. The
  change is entirely in the value model, where the invariant belongs.
- The [P1.3.0 timeline-value note](../feature/2026-08-24-p1-3-0-timeline-value.md) lists
  the op-enforced invariants and is updated in place with the source-window bound and
  the leave-it-valid rule.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
