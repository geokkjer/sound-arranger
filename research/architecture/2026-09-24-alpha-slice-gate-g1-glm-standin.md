# Reviewer gate (stand-in) — alpha slice G1 (markers + clip naming), GLM-5.3

> Research input, 2026-09-24. Co-worker: **GLM-5.3** (`zai/glm-5.3`), read-only, standing in for the
> designated gate **Kimi K3** (unusable: every API run died with no output — see
> [`2026-09-23-kimi-gate-alpha-slices.md`](2026-09-23-kimi-gate-alpha-slices.md)).
> Scope: the plan's item 14 — markers as a value field with a logged vocabulary, clip names as
> labels, the `host v1` spelling of both (including `add_clip`'s new trailing name operand), and the
> shell's keys/ruler/lane drawing. The reviewer drove a path-dependency probe crate
> (`/tmp/g1probe`) over `format_arrange`/`parse_arrange_line`, the clip-editor codec, serde, and an
> end-to-end `host v1` → `save` → `load` round trip, plus every test suite.
>
> **Disposition: `merge with changes`; the must-fix and all three should-fix findings are real and
> are fixed, with tests.** The must-fix was the sharpest kind: a documented behaviour (clearing a
> label) that no surface could reach, and that would have diverged the journal from the value if it
> ever had.

## Must-fix

1. **`RenameClip` with an empty name could not be spelled in the text format.** The formatter emitted
   `rename_clip t0 c0 ` (a trailing space), which tokenises to three words and the parser refused on
   arity — so (a) no user surface could clear a label (the host parser refuses the 3-word form, and
   the TUI's `C` prefill plus a bare Enter hits the unfinished-prefill guard), and (b) because the op
   *is* reachable programmatically (and is unit-tested), an empty name in the history would make
   `save` refuse the whole session while `journal_append` **silently dropped** the entry — a
   value/journal divergence after a crash. **Fixed by giving "clear" its own shape**: the 3-word
   `arrange rename_clip <track> <clip>` clears the label, the 4-word form sets it, and the formatter
   emits exactly the line the parser accepts (pinned by the text round-trip test, which now includes
   a cleared rename). Re-verified end to end at the CLI: a script that sets, renames and clears a
   label saves and reloads without error.

## Should-fix

2. **The ruler's `▼` clobbered bar numbers**, contradicting the slice's own stated rule ("bar numbers
   win the cells they number") — a marker on a bar turned `14` into `1▼4`, and a later marker's glyph
   could truncate an earlier marker's name (`ver▼e`). **Fixed**: the glyph slides to the nearest free
   cell to its right (a marker whose cell is a digit or another glyph, and whose next three cells are
   too, is simply not drawn at that zoom — an absent glyph is honest, a clobbered number is not), and
   names only ever write into empty cells. The no-op "separating space" block is gone.
3. **The lane-label comment contradicted the code.** The comment said "one row above the bottom so
   the envelope's shape still reads" while the code drew on the bottom row. **Fixed by making the
   code match the intent**: the label takes the row above the bottom when the lane has two, so the
   envelope keeps its floor.
4. **The clip-editor codec test was not extended** to the three new ops or to a named `AddClip` — the
   `""` ↔ `None` name convention is exactly what `every_op_round_trips_exactly` exists to pin (the
   `loop_len` `0` convention has its own coverage). **Fixed**: the test now round-trips `RenameClip`,
   `SetMarker`, `RemoveMarker` and an `AddClip` with `name: Some(..)` (the reviewer verified all six
   cases by probe; the test keeps them verified).

## Notes (recorded, fixed where cheap)

5. **A hand-written unsorted `markers` list would silently break `marker_at`'s binary search** (the
   reviewer's probe: `[{100,"z"},{50,"a"}]` → `marker_at(100) == None`). Dormant today (the text
   format can only build sorted lists, and only the retired Tauri shell consumes the serde form), but
   the type should own its invariant: **fixed** with a `deserialize_with` that sorts and
   deduplicates, so the list is normalised whatever it was read from.
6. **Validation asymmetry**: `add_clip` checked the name at parse time while `rename_clip`/`set_marker`
   checked it at apply. Both refuse before logging, so nothing is logged invalidly — but **fixed** the
   bypass that mattered: `validate_clip` now refuses an unusable clip name, which is also what makes
   `Some("")` unrepresentable (so the codec's `""` = no name stays unambiguous).

## Checked and correct (executed, not assumed)

- **`add_clip` name/loop ambiguity: none.** A named clip with no loop prints the loop slot as `0`;
  names that look like numbers (`5`, `0`) reparse as names; loop-only lines (11 words) and bare lines
  (10 words) are unchanged — the reviewer probed every combination.
- **Old sessions load**: a pre-slice `session.txt` (loop-only `add_clip`) loads with `name: None` and
  no markers; an old serde JSON (without `markers`/`name`) deserializes through `#[serde(default)]`.
- **The end-to-end round trip is byte-faithful**: a script with a named `add_clip`, two markers, a
  rename and a marker-rename saves and reloads to exactly the same value, and the saved
  `session.txt` carries the lines in the order that reproduces it.
- **`snap=` and `@frame` still work** on a named `add_clip` (the trailing modifier stripper sees the
  name, not the modifier).
- **`end_frame()` stays clip-based** (a marker past the last clip does not extend an export), markers
  touch no audio path, `SetMarker` keeps the list sorted via the `binary_search` insert, `RemoveMarker`
  on nothing is refused and never logged, and strict `marker_after`/`marker_before` walk the list at
  both ends.
- **The label travels**: chop, razor-split and duplicate all carry it (chop copies `c.name`, split
  clones the clip).
- **The prefill guard**: `'` and `C` prefills end with a space (an unfinished Enter gets the hint),
  `X`'s complete-line prefill still runs as-is.
- **No key conflicts** (`C`, `;`, `'`, `"` were unbound; the keymap tests pass).
