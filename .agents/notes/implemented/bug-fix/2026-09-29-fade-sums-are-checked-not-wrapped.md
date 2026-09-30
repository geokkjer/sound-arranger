# Agent Note: Fade sums are checked, not wrapped

Status: implemented

## Problem

The clip value carries two fade lengths, and the model's rule is a **sum**: `fade_in +
fade_out <= src_len`. Both operands are raw `Frame` (`u64`) all the way from the
`host v1` text format, where the parser takes them as unbound `u64` (`set_clip_fade`
words 3/4, `add_clip` words 7/8 — neither is a `frame_operand`, so `snap=` cannot clamp
them). One line of a script is enough to ask for a pair whose **sum leaves the range**:

```text
arrange set_clip_fade t0 c0 18446744073709551615 1
```

The check was `c.fade_in + c.fade_out > c.src_len` in `validate_clip`, and the same
expression inline in the `SetClipFade` arm of `Timeline::apply_mut`. `u64::MAX + 1`
wraps to `0`, so `0 > src_len` is false and the pair **passed**:

- **debug**: the `+` itself trips the overflow check and panics *inside
  `ClipEditor::apply`'s lock critical section*, so the `Mutex` is poisoned. Every later
  `snapshot()`/`host.arrangement()` returns `Err("timeline poisoned")` and every later
  arrange op is refused — one bad line takes the whole editor down. The panic was
  reachable in production and not caught there.
- **release**: no panic, so the op was **accepted and logged**, and `fade_gain` then
  computed `gin = min(off / 1.8446744e19, 1.0) = 0.0` for every sample. The clip
  rendered **completely silent** while the log, the panel and the clip's gain all claimed
  it was audible, and any later `MoveClip`/`Trim`/`Stretch` re-validated and was refused,
  so it could not even be nudged. The project had documented the debug arm as the poison
  path in the [host-honesty note](../feature/2026-08-27-host-honesty-fixes.md), whose
  test asserted the panic — which is why neither arm was caught.

## Decision

**The sum is computed with `checked_add`, in both places, and an unrepresentable sum is
refused like any other over-long fade.** `crates/media/src/timeline.rs`:

- `validate_clip`: `if c.fade_in.checked_add(c.fade_out).is_none_or(|s| s > c.src_len)`.
  This covers `AddClip` and every op that re-validates (`MoveClip`, `MoveClipToTrack`,
  `Trim`, `Stretch`).
- the `SetClipFade` arm: the same expression against the located clip's `src_len`.

The refusal is the **existing** error (`"fades exceed the clip length"`) — the overflow is
not a new kind of bad fade, it is a pair that does not fit the clip, and it says so.

This is the one choke point. `Timeline::apply` is what the clip editor's live apply, the
gesture probe and the engine's op handler all call (`clip_editor.rs`), so fixing the
value model covers every path — the text format, the gesture/batch path and log replay
alike — rather than one caller at a time. The user-visible outcome is a fail-loud `Err`
on a line a script author can see and fix.

**The report's belt-and-braces suggestion — capping fades at parse (`fade_in.min(src_len)`
in the host's `set_clip_fade`/`add_clip` arms — is deliberately not applied.** It would
silently rewrite what the script asked for (a `u64::MAX` fade-in becomes `src_len`, a
*legal* value) and log a clip the user never wrote. That is coercion, and this codebase
rejects it everywhere else for the same reason: `loop_region` times must fit `u32`,
mixer channels must not be silently truncated, `nan` gain refuses at parse. A refusal
costs the user one line; a silent cap costs them a clip that is not the one they made.

## Alternatives considered

- **Cap the fades at parse (`fade_in.min(src_len)`), as the review suggested.** Rejected:
  silent coercion on a user-authored value, and the log would then record a clip the
  script never asked for. The parse layer cannot know `src_len` for `set_clip_fade` at all
  (the clip is resolved later, by the value model), so the cap could only be applied to
  `add_clip` — leaving the other half of the bug open while making the first half lie.
- **Saturating add (`saturating_add`) instead of `checked_add`.** Rejected: it turns an
  overflowing pair into `u64::MAX`, which is still `> src_len` and so still refused — the
  same outcome, reached by a longer route that hides the intent.
- **Validate the fades inside `fade_gain` at render time.** Rejected: it is the audio
  render path, which must stay bounded, allocation-free and panic-free, and a value-model
  invariant belongs in the value model. A render-time guard would also be *too late* — the
  op is already logged by then, so the log and the value would disagree.
- **Keep the overflow and keep the debug-gated poison test.** Rejected: the test asserted
  the bug as intended behaviour, which is why both arms went unnoticed. It is removed; the
  honest behaviour it stood in for (a refused op leaves the session intact) is now asserted
  directly, in debug and release alike.
- **Add a `validate_fades(fade_in, fade_out, src_len)` helper used by both sites.**
  Deferred: there are exactly two sites and they are four lines apart in one file. A helper
  earns itself when a third caller appears.

## Consequences

- `arrange set_clip_fade t0 c0 18446744073709551615 1` and the `add_clip` equivalent are
  refused by name, in every build, with the timeline left untouched — the clip keeps its
  fades and the next legal edit still applies.
- **No behaviour change for a legal pair.** `fade_in + fade_out == src_len` is still
  accepted (a full-length fade-in is reachable from the shell and the clipboard), and the
  boundary is pinned by the new test.
- Three regression tests, each verified to fail on the unfixed code: the debug panic
  (`attempt to add with overflow` at the `+`), the release silent-acceptance
  (`fade_in u64::MAX + fade_out 1 wraps to 0 — refuse, never admit`), and the editor
  staying usable after the refusal.
- The [host-honesty note](../feature/2026-08-27-host-honesty-fixes.md) is updated in
  place: its `#[cfg(debug_assertions)]` poison test and the `catch_unwind` import are gone,
  since the input that drove them is now refused rather than fatal.
- The render path is untouched: `fade_gain` still divides `off as f32 / fade_in as f32` and
  `min`s to 1.0. It is now unreachable with a fade that cannot fit the clip, which is
  exactly the range where the division was degenerate.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
