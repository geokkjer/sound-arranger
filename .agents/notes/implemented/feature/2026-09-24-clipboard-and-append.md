# Agent Note: the clipboard — copy / cut / paste / append (alpha slice D1)

Status: implemented

## Problem

The arrangement could be built clip by clip and edited in place, but a clip could not be
**reused**: there was no copy, so repeating a bar, doubling a take or moving a phrase meant
`add_clip` with the source id, offsets, fades and gain typed again — exactly the bookkeeping
the tool exists to remove. "Confidently cut and paste audio" is the owner's own phrasing of
the alpha, and half of that sentence did not exist.

The vocabulary was already there, which is what makes this slice cheap: `add_clip` creates a
clip, `delete` removes one, and `group begin … group end` (alpha slice A1) makes a set of ops
**one** gesture — one history entry, applied all-or-nothing.

## Decision

**The clipboard is a shell value; the paste is a logged gesture.**

- `App.clipboard: Vec<(i32, Placed)>` plus `clipboard_origin` (the earliest copied clip's
  start, so a paste keeps the copied clips' relative spacing) and `next_paste`. Copying
  changes nothing, so it is **never logged**; only the paste reaches the log. The clipboard
  **survives a load** (it is a value, as a DAW's clipboard is), so a paste checks the session's
  pool for each source and refuses *before* minting anything when one is absent — named in the
  status — rather than logging a clip the media layer accepts and the panel then drops.
- `y` — copy. The scope is the clips intersecting the **visual selection** on the active
  track, or (no selection, or a selection with no width) the clip under the playhead. A copy
  consumes the selection and returns to normal mode, like the other selection actions. The
  clipboard holds the clip *values*, including `source_id` — a paste is a reference to the
  immutable pool source, never a copy of the audio.
- `c` — cut: the same copy followed by one `group` of `delete` lines, so a cut is one undo
  step and the clipboard still holds the clips for `p`.
- `p` — paste at the playhead on the active track; `P` — paste **appended** after that
  track's last clip (the arrangement's end when the track is empty). Append needs no new op
  at all: the target frame is computed in the shell, which is what the plan predicted.
- The paste is one `group` of `add_clip` lines with **minted ids** (`paste.{n}`), following
  the `chop` precedent: the shell starts `n` above any `paste.N` the session already names and
  never reuses one within a run. A collision is refused whole, before anything is built — the
  host's group is atomic anyway, but a silent no-op would look like a broken key.
- **New boundaries get a default micro-fade** (`MICRO_FADE` = 64 frames ≈ 1.3 ms at 48 kHz —
  long enough to kill the click, short enough not to be a fade). A *copied* fade is kept
  instead, and the **guard is what gives way** when the pair is tight: each bare side is capped
  at `src_len - the other side`, so the model's `fade_in + fade_out <= src_len` rule holds for
  every combination — including a legal `fade_in = src_len` (reachable with `f` at the clip's
  end), where the pasted clip simply gets no fade-out at all. A one- or two-frame clip has no
  room for a guard either (its half is 0), which the note's earlier "capped at half the clip"
  overstated; the *paste* is still legal, it is the click guard that is absent.
- The paste target is the snapped playhead (the grid applies to edit targets like every other
  edit), and it is deliberately **not** clamped to the arrangement: a paste past the end is how
  the piece grows. (A *seek* is clamped — past the end there is nothing to show.) An **append**
  target is the track's last clip end, so the snap note belongs to the playhead branch only:
  comparing an append target to the playhead is a number that means nothing.
- **A gesture announces success only when the host applied it.** `App::command` returns whether
  the outcome was `Ok`, and `cut`/`paste`/`trim_to_selection` set their status line only then —
  otherwise a refusal ("arrange refused: …") is overwritten by "pasted 1 clip", which is exactly
  how a refused edit looks like data loss (found by the slice's gate, along with the fade-pair
  bug above).

## Alternatives considered

- **A `paste` op in the media vocabulary.** Rejected: it would carry a list of clips as an
  operand (a new wire shape) for semantics that `add_clip` + `group` already have exactly.
  The plan's "paste is a compound gesture" is the cheaper and *more* inspectable form: the
  saved session shows the clips it created, not a paste command that has to be re-expanded.
- **A media-side register (a clip id the log points at).** Rejected: it puts a UI convenience
  into the document, and a saved session would depend on clipboard state that is not saved.
- **Copying the audio into the pool on paste.** Rejected: the pool's sources are immutable
  session material and a paste is a *reference* to one; duplicating bytes would multiply disk
  use per paste and break the "a clip is a straight read of a source" story.
- **Snapping every pasted clip independently.** Rejected: it would destroy the copied clips'
  relative timing (a phrase pasted at a bar line must stay a phrase). One snap, on the paste
  origin, then offsets.
- **Clamping the paste target to the arrangement.** Rejected after a test caught it: the
  composer's most common paste is *at the end* (append), which is by definition past the
  current end. Seeks clamp; pastes extend.
- **Copying the rendered audio through the OS clipboard.** Deferred: the alpha's clipboard is
  inside the arranger (clip references, not samples). Handing audio to another program is the
  "no sidecars, drive external programs" work, not this slice.
- **A multi-track selection.** Deferred: the clipboard's shape already carries a per-clip
  **track delta** (all zero today, because a selection is one track), so a multi-track
  selection is a change to the *copy*, not to the clipboard or the paste.

## Consequences

- A phrase can be copied once and placed anywhere, on any track, at the playhead or appended,
  and the whole paste is one `u` away from gone. The session log shows the clips that were
  created — the same `add_clip` lines a script would write, with the same ids.
- The clipboard is per-run shell state (like the grid): it is not saved, but it *does* survive a
  load in the same run. That is deliberate — it is a value, not document state — and a paste
  whose source the new session's pool lacks is refused with the id named.
- Tests: a copy takes the clip under the playhead with its source id and pastes it as
  `paste.1` with micro-fades, and **one** undo removes the pasted clip; a cut removes the clip
  in one gesture that one undo restores; append lands the clip after the track's last clip
  (`192 000` for a track ending at `144 000`); a grid-armed paste lands on the bar line and
  reports the snap (`snapped from 150000`), which is also the test that caught the paste-target
  clamp; a **multi-clip** selection pastes both clips with their spacing kept and one undo
  removes both; a **tight copied fade pair** (`fade_in` 60 of a 100-frame clip, and a
  full-length `fade_in`) pastes legally; and a **refused gesture** reports the refusal instead
  of claiming success, with an out-of-session source refused by name before anything is minted.
- A multi-clip paste is one gesture because every `add_clip` member carries no `@frame`, so the
  host's "one frame per group" rule is satisfied trivially — a property of the *group* contract,
  now covered by the multi-clip test rather than by inspection.
- `Placed` gained `source_id` (`media::Clip::source`): the panel drew clips without ever
  needing the id, but a *paste* must name the source, so the value the panel holds now carries
  it. The alternative — looking the id up by comparing `Arc` pointers — is a bug waiting to
  happen.
- **Still open**: a multi-track selection (the delta is in the shape, the selection is not);
  pasting between *sessions* (the clipboard does not survive a load, and the source may not
  exist in the new pool — a paste naming an absent source fails loudly through the arrange
  reconcile, which is the right failure but not a friendly one); a "paste at the original
  position" variant; OS-clipboard audio.

## The gate's findings

The slice's independent gate returned `merge with changes`; its two must-fix findings were real
and are fixed here, with tests
([review + disposition](research/architecture/2026-09-24-alpha-slice-gate-d1-glm-standin.md)):

1. **The fade guard did not guarantee the model's sum rule** when a copied fade was kept on one
   side (`fade_in = 60` of a `src_len = 100` clip left only 40 frames, but the guard asked for
   64), so the paste minted an `add_clip` the host refused. Fixed: the guard is capped against
   the *other* side, in a fixed order (in, then out), so the pair is always legal.
2. **A refused group still announced success** — `paste` overwrote the host's refusal with
   "pasted N clips" after burning its minted ids, so a refused paste looked like data loss.
   Fixed: `command`/`arrange_group` report whether the host applied the command, and every
   gesture with its own success message sets it only then.
3. **The append status lied about snapping** (it compared the playhead to an append target).
   Fixed: the snap note is emitted only for the playhead branch.
4. **The clipboard survives a load** (the note claimed otherwise) and a paste naming a source the
   session's pool lacks was logged and then dropped by the panel. Fixed: the shell keeps the
   pool's source ids and refuses the paste with the missing id named, before minting anything.
5. **Multi-clip paste was untested** (it rests on group members carrying no `@frame`); a
   multi-clip test now covers spacing and one-undo-removes-both, and the one- or two-frame
   clip's absent guard is stated rather than overclaimed.

The gate also reported a **pre-existing flake** outside this diff:
`host::tests::a_take_records_into_the_pool_and_plays` failed once under a parallel workspace run
(its fixed 120 ms drain sleep is a race on a loaded machine). It is fixed as its own commit
rather than smuggled into this one.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-24.
