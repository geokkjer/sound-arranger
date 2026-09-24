# Agent Note: markers and clip names — a long piece becomes navigable and recognisable

Status: implemented

## Problem

The plan's item 14: "**Markers/sections and clip naming** (logged lines + shell list + keys) so a
30-minute piece is navigable and recognisable".

The product records *long live jams*; after half an hour of arranging, the shell shows a wall of
`c17`/`c42` on anonymous lanes, and the only way to reach a section is to remember its frame number
or scrub by ear. Everything else in the alpha assumes the piece is *findable* (the export renders
"the whole arrangement"; the pool panel places material by id) — this is the slice that gives the
timeline a human index.

Two questions had to be answered before any code:

1. **Where do markers live?** A marker is *document* state (it must replay, save, and undo like any
   edit), so it belongs in the arrangement value — not in a shell-side list, which would drift from
   the log and vanish on a seek.
2. **What is a clip name, exactly?** The clip's `id` is its address (every op keys on it, ids are
   machine-minted); a name is a *label*. Making the name an address would break the log's
   determinism, so the name carries no semantics — but it still has to be **spellable by the
   `host v1` format**, because the format is the save file.

## Decision

**Markers are a value field, set by an op.**

- `media::Marker { at_frame, name }`, and `Timeline.markers: Vec<Marker>` with `#[serde(default)]`
  (an older saved session still loads; an unmarked session is an empty list).
- The list is **sorted by frame with at most one marker per frame**. That is what makes the write op
  `SetMarker { at_frame, name }` *add or rename in one* — it can never fail on a duplicate, and the
  log records one fact per gesture ("this point is called X now"). `RemoveMarker { at_frame }` is
  refused when there is nothing there: a deletion that deletes nothing is not an edit.
- Navigation is value-level, so the shell and any other reader share it: `marker_at`,
  `marker_after` (strictly after), `marker_before` (strictly before). Strictness is what makes
  repeated `next`/`prev` walk the list instead of standing still on the marker they just reached.
- **Markers never touch the audio.** The render path does not read them, and `end_frame()` stays
  clip-based — so a marker past the last clip does not make an `export` render trailing silence, and
  the bytes of a mix are identical with or without markers.

**Clip names are labels.**

- `Clip.name: Option<String>` (`#[serde(default)]`), set by `RenameClip { track, clip, name }`; an
  **empty name clears it** (a rename vocabulary that could only add labels would make "unnamed
  again" unexpressible). It survives a chop (the pieces keep their source's label) and every op that
  moves a clip keeps the clip — so the label travels with it and `undo` restores it.
- Every op keeps keying on the **id**: a name cannot address anything, two clips may share one, and
  changing a name cannot move audio. ([The media-pool note](2026-08-24-p1-3-3-media-pool.md)'s
  "an id the log cannot name back is not an id" rule applies to labels too — see below.)

**Names must be log-spellable.** `media::valid_name` (one whitespace-free token, not starting with
`@` or `snap=`, no `#`) validates track ids, clip names and marker names alike; `valid_track_id` is
now a thin alias. The format is whitespace-separated with **no quoting**, so a name with a space in
it would save a session that cannot be reopened — the refusal says so, and the shells' prompts say
"one word" (dashes are fine: `bridge-take-2`).

**Vocabulary and keys.**

- `arrange set_marker <at_frame> <name>`, `arrange remove_marker <at_frame>`,
  `arrange rename_clip <track> <clip> <name>` — plus `add_clip`'s **optional trailing `name`**
  operand (after the optional `loop_len`, printed as `0` when a name is present without a loop, so
  the line reparses as the same clip).
- Shells: `'` names a marker at the playhead (the command line opens prefilled
  `arrange set_marker <frame> ` — the name is the missing word, the same prefilled-prompt shape as
  `R`'s track rename and `X`'s export), `;` / `"` jump to the next / previous marker, `C` names the
  clip under the playhead.
- The **ruler draws each marker** (`▼` plus its name where the cells are free — a bar number is never
  overwritten, because the ruler's numbers are what make the bars readable), the **lane draws the
  clip's label** (name, else id) where the clip is wide enough for the whole word, and the
  **transport readout names the marker the playhead is on**, so the section you are in is on screen.
- A marker jump lands **exactly** on the marker's frame, not on a snapped frame: snapping it would
  move the very point the key exists for.

## Alternatives considered

- **Markers as a shell-side list** (a TUI `Vec<(u64, String)>`). Rejected: it would be a second copy
  of document state — it would not replay, not save, not undo, and would silently reset on a seek
  (which rebuilds the session). The whole point of the log-is-the-document rule is that a
  user-visible fact has one home.
- **`AddMarker` + `RenameMarker` + `MoveMarker`** (the symmetric set). Rejected: three ops for one
  invariant. At most one marker per frame makes `SetMarker` sufficient — it cannot fail, which is
  also what keeps a gesture one log line.
- **Markers keyed by name instead of frame** (`marker <name> @frame`). Rejected: a name would then
  have to be unique and stable (a rename would be an address change), and a section boundary is
  fundamentally a *time*.
- **A marker *offset* from a clip** ("the bridge of take 2"). Rejected as premature: sections outlive
  the clips they were marked on in practice, and the plan asks for navigation, not for anchors.
- **A quoted name syntax in the format** (`name="two words"`). Rejected for alpha: the parser is
  space-split by design (the v1 format's whole virtue is that a session is a list of words), and the
  constraint lands on the shell's *prompt* rather than on the format. Recorded as the way to lift it
  later, if names with spaces become worth it.
- **A free-text clip name** (any characters). Rejected for the same reason: a name that cannot be
  written to `session.txt` is a session that cannot be reopened. The refusal names the constraint.
- **Renaming by index rather than id** (`rename_clip 3`). Rejected: ids are the log's stable
  addresses; an index is a shell concern (which is why `C` resolves "the clip under the playhead" to
  an id before building the line).
- **A dedicated markers panel** (a fourth focus ring stop). Kept open: the ruler glyphs + names and
  the readout cover navigation at alpha scale ("shell list" in the plan is satisfied by the ruler's
  labelled marks and the jump keys). A list/outline view is the natural beta feature if pieces get
  long enough to want one.
- **Snapping a marker jump to the grid.** Rejected: a marker is an exact point; `[`/`]` already
  exist for grid stepping.

## Consequences

- **A 30-minute piece has an index**: name the sections you care about while arranging, jump between
  them with two keys, and see the clip's label on its lane — no frame arithmetic.
- **The value's schema grew by two defaulted fields** (`Timeline.markers`, `Clip.name`). Both are
  `#[serde(default)]`, so a session saved before this slice loads unchanged; the *text* format grew
  one optional operand (`add_clip`'s trailing name) and three new ops, which a pre-slice parser
  would refuse loudly rather than misread (the arity check) — the honest failure mode for a format
  with one version.
- **The log stays one fact per gesture**: a marker set is one `arrange` line, a clip rename is one,
  and both undo in one step.
- **Markers are inert to the audio**: a mix's bytes are unchanged by any marker, which is why they
  are safe to add to an existing session.
- **Still open**: a markers *list* view (see above); a name for a *track* beyond its id (the id is
  what the mixer channel and every op use, and `RenameTrack` already renames it — a separate label
  would be a third name for the same thing); markers that move an *edit* rather than the playhead
  (select the range between two markers, render it); and carrying a marker's name into the pool when
  a clip is exported as material.

## The gate's findings

The slice's gate returned `merge with changes` ([review +
disposition](../../../../research/architecture/2026-09-24-alpha-slice-gate-g1-glm-standin.md)); the
must-fix and all three should-fix findings were real and are fixed, with tests:

1. **Clearing a label was unreachable and lossy** (must-fix) — the formatter printed
   `rename_clip t0 c0 ` (a trailing space), which parses as three words and was refused, so no
   surface could clear a name; and because the op is reachable programmatically, an empty name in the
   history would have made `save` refuse the session while the journal **silently dropped** the entry
   (a value/journal divergence after a crash). Fixed by giving "clear" its own shape: the 3-word
   `arrange rename_clip <track> <clip>` clears, the 4-word form sets, and the formatter emits the
   line the parser accepts.
2. **The ruler's glyph clobbered bar numbers** (should-fix) — `14` became `1▼4`, and a later
   marker's glyph could truncate an earlier marker's name. Fixed: the glyph slides right to the
   nearest free cell (or is not drawn at that zoom), and names only write into empty cells.
3. **The lane-label comment contradicted the code** (should-fix) — it claimed the row above the
   bottom while drawing on the bottom; the code now matches the intent.
4. **The codec test was not extended** (should-fix) — `every_op_round_trips_exactly` now covers the
   three new ops and a named `AddClip` (the `""` = no-name convention, the twin of `loop_len`'s `0`).
5. **An unsorted deserialized marker list** would have broken `marker_at`'s binary search (note) —
   fixed with a `deserialize_with` that sorts and deduplicates, so the type owns its invariant.
6. **`add_clip` bypassed the name validation** (note) — `validate_clip` now refuses an unusable clip
   name, which also makes `Some("")` unrepresentable.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-24.
