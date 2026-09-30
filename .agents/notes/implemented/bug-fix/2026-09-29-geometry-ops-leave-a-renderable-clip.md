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

**The verification of the fix found three more instances of the same class, in the arms
the fix touched, plus a test that did not test what it claimed.** All four are below,
because the rule the fix states ("an op that rewrites a clip's geometry leaves the clip
satisfying `validate_clip`, and no sum it computes can wrap") was true on the arms it
named and nowhere else:

- **The forward chop still summed `src_start` raw** (`src_at += slen` at the end of the
  piece loop). The fix had made the *reversed* chop's seed a `checked_add` and left the
  forward walk alone — the same debug panic and the same release wrap, one boolean
  (`reversed`) away from the fixture the fix added. The route is a clip that reaches an
  op without passing `AddClip`: a hand-built `Track` is a legal `&self` to
  `Timeline::apply`, and a `Timeline` is `Deserialize`.
- **`LoopRegion { times: 1 }` on an already-looped clip shrank it silently**, and the
  new `validate_clip` blessed the result. `times` was never *defined* — the op read
  `region = loop_len.unwrap_or(src_len)` and set `src_len = region * times`, which is a
  pass count on a fresh clip and a **factor** on a looped one, so the same number meant
  two different things depending on the value it landed on. Nothing in the code, the op's
  doc or the `host v1` text said which.
- **`Clip::end()` was an unchecked `+`** on a `pub` method whose doc *asserted* the span
  had been validated on admission. `Timeline::end_frame()` calls it over every clip, and
  `HostSession::export` calls `end_frame` for the length it renders — so an arrangement
  holding one unrepresentable span (again: a hand-built value, or a snapshot) panicked in
  debug and, in release, produced a **wrapped** end: a mix silently missing its last
  clip. The doc was the load-bearing part and it was false for every clip that did not
  come from an op.
- **The host test's audio assertions did not bind to the fade.** It asserted
  `audio[0]` is quiet and `audio[1000] > 0.05`: the ramp source is 0 at frame 0 whatever
  the gain, and frame 1000 is fully open under *any* fade shorter than 1000 frames, so
  the assertions passed with the fade uncapped, capped to the wrong length, or dropped
  entirely. A test that claims to prove the split is audible proved only that some audio
  came out.

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

**An op checks the arithmetic it does and the clip it was handed** — the rule the first
pass of this fix only applied to its outputs. `Timeline` is `Deserialize` and a
hand-built `Track` is a legal `&self` to `apply`, so "the clip was admitted" is not
something an arm can see. So:

- `ChopClip` calls `validate_clip` on the clip it is about to re-tile, **before** the
  walk, and the walk itself is checked in all three places it moves: the reversed
  `src_at -= slen` is a `checked_sub`, the forward `src_at += slen` and the timeline
  `at += slen` are `checked_add`s. The refusal names the window
  (`"chop: clip 'c0' source window overflows"`) rather than a piece that would not fit.
  (The other geometry arms' remaining sums are already safe: `Trim` goes through
  `add_signed`, and every arm validates the value it produces.)
- The module doc states the rule, so the next arm inherits it: an op's sums are `checked`
  and it validates the clip its own arithmetic depends on.

**`Clip::end` is `Option<Frame>` and `Timeline::end_frame` is `Result<Frame, String>`.**
`end()` is `at_frame.checked_add(src_len)`, so the doc's claim becomes checkable instead
of asserted, and `end_frame()` refuses an arrangement that holds an unrepresentable span
**by name** (`"clip 'c0' span overflows the timeline"`), which is what `export` now
reports instead of a wrapped length. Two callers do not propagate the failure and do not
need to: `ArrangerNode::render` saturates, because `ArrangerNode::new` validated every
clip it holds and a block loop must stay *total* (a panic there stops the render, a
wrapped end clips the block short), and `RazorSplit`'s bound check reads the checked
end, so a clip with an unrepresentable span is refused there by name rather than compared
against a wrapped one.

**`LoopRegion.times` is the number of passes, not a factor — and a shorter re-bake caps
the fades.** The region is fixed by the first bake (`loop_len`, or the clip's own length
on the first loop), so a re-bake re-reads *the same* region and sets the total to `times`
passes of it: `times: 3` applied twice is three passes, not nine, and `times: 1` is the
single pass the clip was before its first bake. A re-bake with a **smaller** count
therefore shortens the clip — that is what the number says, it is now written down in the
op's doc, the `host v1` form and this note — and a shrink caps the fades to the length
they now cover, exactly as `RazorSplit`, `ChopClip` and `Stretch` do. Without the cap
the newly added `validate_clip` *refused* the op (`"loop region: clip 'c0' fades exceed
the clip length"`): a shorter loop of a fading clip could not be made at all, which is
the same class of bug from the other side. `times: 0` stays refused — it is not
"un-loop", and un-looping is not an op in this model (delete and re-add is).

**The host test proves the fade by its gain, not by a sample being non-zero.** The mix is
scaled by a constant on the way out (a mono source spread across the stereo bus), so the
assertion compares frames that read the **same source value** and the constant cancels.
The ramp repeats every 257 frames, and 500 ≡ 757 ≡ 1271 ≡ 243 (mod 257), so the left
half (frames 0..1200, `fade_in` 1200) at frames 500 and 757 over the unfaded right half
at frame 1271 *is* the fade's gain at those frames: 0.4167 and 0.6308. A second pair of
right-half frames at the ratio of their source values pins that half as unfaded, which is
what makes the reference a reference. The wrong fades that fail it are named in the
test: uncapped (0.125, 0.189), capped to 600 (0.833, 1.0), dropped (1.0, 1.0), and
equal-power (0.645, 0.795) — the last verified by changing `fade_gain`'s curve with
every value assertion still green.

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
- **One admission check in `apply_mut`, for every op that names a clip.** The strongest
  version of the rule, and the natural next step if a snapshot route ever lands.
  Rejected *here* because it would change the refusal of ops this fix does not touch (a
  `set_clip_fade` or a `rename_clip` on a snapshot clip would start failing with a clip
  error), and because each arm that depends on its input now has the check where its own
  arithmetic depends on it.
- **Read `times` as a factor, and leave the shrink as the thing it is.** Rejected: it is
  the reading the code already had, and it is the one that makes the *same* number mean
  two things (`times: 1` grows nothing, `times: 2` doubles a clip) depending on the value
  it lands on. A count is predictable from the op alone, and the log round-trips the
  resulting value exactly, so the two readings differ only in what a reader expects.
- **Refuse a re-bake of a looped clip outright** (the "loop phase is not representable"
  family). Rejected: there is no representational obstacle — `LoopRegion` is the op that
  *created* the loop — and the refusal would make the pass count inexpressible after the
  first bake, which is a capability the model has today.
- **Treat a re-bake to the current count as a no-op and read `times: 0` as "un-loop".**
  Rejected: `times: 0` is refused today and a script that means "un-loop" is not asking
  for a loop, and an un-loop has no representation here (a `loop_len` of `None` is the
  contiguous read, and dropping one rewrites material the user placed). Re-deciding this
  wants its own note, when an un-loop is asked for.
- **Keep `Clip::end() -> Frame`, saturating, and guard the export.** Rejected: it
  publishes `Frame::MAX` as a legitimate arrangement end — an `export` would then either
  try to render 5.8e11 years or need a second guard anyway — and the `pub` doc's claim
  stays a claim. `Option` makes the claim checkable, and `Result` on `end_frame` is what
  lets the *caller* refuse with the clip's name.
- **Normalise clips in `Deserialize`, the way markers are normalised.** The precedent is
  in the same struct, and it is the right answer for a *sort order* a binary search
  depends on. Rejected for now: a span cannot be normalised without silently rewriting
  data (which of `at_frame` and `src_len` moves?), and the checked accessor already
  refuses by name. The workspace has no snapshot route yet — nothing deserializes a
  `Timeline` — so the decision is open; if one lands, it belongs here.
- **Assert a fade with a non-zero sample, at a louder frame.** Rejected: the ramp's own
  scale makes any "is it audible" threshold satisfiable by a wrong fade. Only a *ratio*
  against the same source value at a different gain is a statement about the fade.

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
- A `loop_region` with a **smaller** `times` on a looped clip now shortens the clip and
  caps its fades; before the cap it was refused whenever a fade covered the longer
  length. Re-baking the same count is the same value.
- **No behaviour change for a fade that fits what it lands on**: a fade no longer than the
  half (or the piece) rides the split or chop untouched, and the interior chop seams are
  still hard.
- `Clip::end()` returns `Option<Frame>` and `Timeline::end_frame()` returns
  `Result<Frame, String>`. `export` therefore refuses an arrangement holding an
  unrepresentable span, naming the clip, where it used to panic in debug and wrap in
  release. The render path is unchanged in behaviour: `ArrangerNode::new` already refused
  such a clip, so the saturating fallback in `ArrangerNode::render` is unreachable for a
  node the platform built.
- **Nine regression tests.** The five from the first pass: three value-model tests, each
  verified to fail on the unfixed value — `a_razor_split_of_a_long_fade_leaves_two_valid_halves`,
  `a_chop_of_a_long_fade_leaves_pieces_the_model_accepts` and
  `a_source_window_past_the_frame_range_is_refused_not_wrapped` — one assertion added to
  `loop_region_bakes_repeats_with_wrap_len` for the growth arm, and
  `a_razor_split_of_a_full_length_fade_still_bounces_audio` in
  `crates/host/tests/arranger_commands.rs`, which **plays** the two-keypress sequence end
  to end (`bounce` goes through `wire_arranger`, so it fails on the unfixed value with the
  real `arranger: clip 'cL' fades exceed the clip length`). Four from this round, each
  verified against the unfixed value: `a_forward_chop_that_would_wrap_the_source_window_is_refused`
  (panics with `attempt to add with overflow` on the raw forward `+=`),
  `a_re_baked_loop_sets_the_pass_count_and_caps_its_fades` (fails with
  `loop region: clip 'c0' fades exceed the clip length` without the cap),
  `a_span_that_cannot_be_represented_has_no_end` (panics with `attempt to add with
  overflow` on the unchecked `end`), and the rewritten audio assertions of
  `a_razor_split_of_a_full_length_fade_still_bounces_audio` (measured 0.645 against the
  0.4167 an equal-power fade gives, with every value assertion still green).
- The render path is otherwise untouched: `fade_gain` and `ArrangerNode::new` are as they
  were. The change is in the value model, where the invariant belongs.
- The [P1.3.0 timeline-value note](../feature/2026-08-24-p1-3-0-timeline-value.md) lists
  the op-enforced invariants and is updated in place with the source-window bound, the
  leave-it-valid rule, the admission rule, the `times` contract and the checked `end`.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
