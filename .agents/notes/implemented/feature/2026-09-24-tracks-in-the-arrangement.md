# Agent Note: tracks in the arrangement — add / rename / delete / reorder (alpha slice D2)

Status: implemented

## Problem

A track could be created (`add_track`) and dropped (`remove_track`), and nothing else. It
could not be **renamed** (a session read `t0`, `t1`, `t2` forever, however the music was
organised) and could not be **reordered**, which matters more than it sounds: a track's
*position* is its mixer channel — the arranger wires track `ti` to `ch{ti}` — so reordering is
how a user regroups the console, and it was impossible without moving every clip by hand.
There was also no key for any of it, and `remove_track` silently dropped the track's clips.

## Decision

**Two new ops in the arrangement value, and three honest gestures over them.**

- `ArrangeOp::RenameTrack { track, to }` — renames in place. The **index** is what feeds
  `ch{ti}`, so a rename never moves the audio: the same track keeps its mixer channel. The new
  name must be a plain, whitespace-free word (`valid_track_id`: the id is written into
  `host v1`, so anything else would produce a session that cannot be parsed back) and must not
  already exist.
- `ArrangeOp::MoveTrack { track, index }` — moves a track to a 0-based index, shifting the
  others. The clips travel with the track (they live inside it), the mixer channel each track
  feeds follows its position, and an out-of-range index or an unknown track is refused. The
  index is **absolute**, not a swap or a delta: replaying the log twice lands in the same
  order, which a relative op would not guarantee in a gesture.
- Both are ordinary logged ops (`EncodeOp`/`DecodeOp` + `ALL_OPS` registration, the host's
  parser and formatter, the round-trip test), so a renamed and reordered session saves,
  reloads and replays to the same value — verified end to end by
  `a_renamed_and_reordered_session_reloads_in_order`, which saves a session whose log holds
  `rename_track t0 lead` and `move_track lead 1`, loads it back, and asserts the order
  (`t1`, `lead`) and that the clip is in `lead`.
- **Keys** (`a`, `R`, `D`, `{`, `}`):
  - `a` adds a track, minting the **first free** `t{n}` (the shell mints ids because they are
    logged), and makes it the active track — you just made it, so that is where the next clip
    goes. The status names the mixer channel the track will feed, and when that channel does
    not exist (the value op is permissive; `track ti` feeds `ch{ti}`, and the mixer's width is
    a mount parameter) it says *that* instead, with the command that widens the mixer — the
    alternative is a track that renders nothing until the transport fails.
  - `R` opens the command line **prefilled** with `arrange rename_track <track> `, so the name
    is typed through the host's own format. One prompt, one parser, no second text widget to
    keep in sync — the modal note's "a widget away, not a project" applied to a rename. The
    prefill is remembered, so walking the history and coming back to the live line returns it
    rather than a blank line, and Enter on an unfinished prefill gets a hint ("needs the new
    value after the last space") instead of the parser's operand count.
  - `D` deletes the active track **and its clips** as one `group`: the op drops the track, so
    the clips have to go explicitly, and one `u` brings the whole lot back. The status says how
    many clips went, so a delete is never a silent amputation.
  - `{`/`}` move the active track up/down; the **active index follows the track**, so the view
    stays on what moved. At the edges the shell says "already first/last" instead of sending a
    command that would be refused.

## Alternatives considered

- **Deleting a non-empty track silently** (the op's own behaviour). The op keeps its semantics —
  a value op that drops a track is what `remove_track` *is* — and the **gesture** is made
  honest instead: the shell deletes the clips explicitly, in the same `group`, and reports the
  count. Changing the op would have made a simple operation conditional and surprising.
- **A separate track table or a second log.** Rejected: the arrangement value already owns
  tracks, and a second representation is exactly what the log-as-document rule forbids.
- **A dedicated rename text widget.** Rejected: the `:` prompt already types `host v1`, and a
  prefilled line is both less code and self-teaching (the user sees the command that renames).
- **Reordering as remove + add.** Rejected: `remove_track` drops the clips, so a "move" would
  destroy audio; and a `group` of clip moves cannot express a track reorder at all.
- **A relative reorder (`move_track t0 +1`) or a swap (`swap_tracks t0 t1`).** Rejected: a
  relative op is not idempotent under replay (the log is the document, and replay must land in
  one state), and a swap cannot express "move to position 3". An absolute index is deterministic.
- **Validating track ids in `add_track` too.** Not done here — see the debt below: the *new*
  op validates, and the pre-existing hole is recorded rather than quietly half-fixed.
- **Showing the track name on its mixer strip.** Deferred: the console's labels are `ch{ti}`
  today, and naming a strip means threading track names into the mixer panel (its own change).

## Consequences

- A session can now be organised the way the music is: name the tracks, order them, and the
  console follows the order (the mixer's channels are the tracks, in order).
- Tests: `media` covers rename (position kept, clips kept, a taken name / a whitespace name / an
  empty name / an unknown track refused) and move (order swapped, clips carried, own-index a
  no-op, out-of-range refused); `host` covers the two ops in the text round-trip and the parser;
  the TUI covers add (first free id, activated, one undo), rename (the prefilled prompt typed to
  completion), reorder (the order and the active index follow; the edges report), and delete
  (the track *and its two clips* in one gesture, restored by one undo).
- `workflow`'s vocabulary test proves the four new `command_name`s (`add_track`,
  `rename_track`, `remove_track`, `move_track`) are ops the host's parser accepts, so a binding
  cannot drift from the log.
- The iced shell reports the new keys through its catch-all ("needs the timeline") rather than
  silently dropping them.
- **Track ids are validated on the way in and on the way across**: `valid_track_id` rejects
  anything the `host v1` format cannot carry *back* — a whitespace-bearing name, an empty one, a
  leading `@` (the `@frame` token), a leading `snap=`, and any `#` (the comment marker) — and
  **both** `AddTrack` and `RenameTrack` use it. The gate found the first draft validated only the
  rename, which left a library-level hole (`Timeline::apply` could name a track something the
  text format strips, and then the save's self-check would refuse *every* save). The host parser
  could not express such a name; the engine API could.
- **Still open**: the mixer strips are labelled `ch{ti}`, not by track name (naming a strip
  means threading track names into the mixer panel); `active_track` is a **cursor**, not document
  state, so undoing a move or a delete leaves the highlight on the index it was on rather than
  following the track that moved — restoring it would need the shell to remember a track id per
  gesture, and the cursor is deliberately not part of the value.

## The gate's findings

The slice's gate returned `merge with changes`; both should-fix findings were real and are fixed
here ([review + disposition](../../../../research/architecture/2026-09-24-alpha-slice-gate-d2-glm-standin.md)):

1. **`a` announced a mixer channel that may not exist** — the value op is permissive, but
   `track ti` feeds `ch{ti}` and the mixer's width is a mount parameter, so adding the fifth track
   to a four-channel mixer produced a track that rendered nothing (and stopped the transport at
   the next play). Fixed: the status says the mixer is too narrow and names the widening command.
2. **The note's end-to-end claim had no committed test** (the host round-trip test only formats and
   parses; it never *applies*). Fixed with `a_renamed_and_reordered_session_reloads_in_order`.
3. `valid_track_id` under-delivered against its own doc comment (reserved tokens could not
   round-trip) and `AddTrack` did not validate at all — both fixed.
4. The note's `add_track` example was wrong (the *host parser* cannot express a two-word track id;
   the hole was library-level) — corrected, and the hole closed.
5. The no-arrangement branches of `D`/`{`/`}` returned silently while `a`/`R` explained
   themselves — fixed.
6. A prefilled prompt lost its prefill on a history walk, and an unfinished prefill produced a raw
   arity error — the prefill is remembered and the unfinished line gets a hint.
7. `active_track` is not restored by undo — recorded above as deliberate (the cursor is not
   document state).
8. `Action::TrackMove` sat next to the pre-existing `Action::MoveTrack` (a *clip* move) — renamed
   to `ReorderTrack`, so the two are no longer a readability trap.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-24.
