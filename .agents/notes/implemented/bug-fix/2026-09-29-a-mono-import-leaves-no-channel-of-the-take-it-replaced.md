# Agent Note: A mono import leaves no channel of the take it replaced

Status: implemented

## Problem

`Pool::import` splits a multi-channel file into `{id}.ch0`, `{id}.ch1`, … and hands those ids
back in `Import::ids()`, so a shell places clips on them ([stereo-pool
note](../feature/2026-09-24-stereo-material-per-channel-pool.md)). Importing **over** one of
those ids replaces the material, and `replace_sources` (`pool.rs`) does that by removing the
whole-file source `{id}.wav` plus every `{id}.ch{k}` at or beyond the number of channel names
the incoming import is about to write.

The mono arm passed its **channel count** (`1`) as that keep-count. But mono material lands as
`{id}.wav`, not as a channel — it writes no `{id}.ch{k}` at all — so `k >= 1` **keeps `ch0`**.
The order that shows it: import a stereo `jam.wav`, so the pool holds `jam.ch0.wav` and
`jam.ch1.wav` and the shell has placed clips on both; then import an unrelated mono `jam.wav`
over the same stem. `replace_sources("jam", 1)` removed `jam.wav` and `jam.ch1.wav` and left
`jam.ch0.wav` — the **replaced stereo file's left channel** — addressable under an id the pool
itself had handed out. The clip on `jam.ch0` went on playing the take the user had just
replaced, silently, and the index listed both `jam.ch0` and `jam`.

This is precisely the failure the stereo-pool note recorded when `replace_sources` was added
("a clip on that id would keep playing audio the user just replaced"); the argument was simply
the wrong number on one arm. Reported as MAJOR #5 by the [Space Bunny
review](../../../../research/architecture/2026-09-29-space-bunny-review-3-media-dsp.md).

## Decision

**A mono import keeps no `{id}.ch{k}` name, so it passes a keep-count of `0`.** The parameter
counts the channel names the incoming material writes: a multi-channel import overwrites
`ch0..channels-1` and keeps exactly those, a mono import writes none, so `pool.rs` calls
`replace_sources(&id, 0)` and every sibling an earlier split left under the id goes with the
whole-file source.

The multi-channel arm is unchanged and stays as it was: `ch0..channels-1` are about to be
renamed over, so keeping them is correct (and it is what the Windows-replacing-a-destination
behaviour and the delete-then-rename fallback in `write_source` already handle).

## Alternatives considered

- **Special-case the predicate instead** (`k >= keep_channels || keep_channels == 1`, the
  report's alternative). Rejected: the same behaviour in a wider spot, and it puts the mono
  special case inside the predicate rather than at the call site that introduced it. `0` states
  what is true — this arm writes no channel names — and leaves the predicate as one comparison.
- **Delete the siblings after the mono commit rather than inside `replace_sources`.** Rejected:
  "importing replaces the id" is implemented in `replace_sources`, and moving the deletion to the
  call site gives two call sites two orders to keep in step for no gain. (The delete-before-commit
  *ordering* is a separate open finding; it is not decided here.)
- **Refuse the clash** ("`jam` already holds a split"). Rejected: importing over an existing id is
  the documented, intended replacement; a mono file whose stem collides with an earlier split is
  an ordinary re-import, not an error, and refusing it is the same silent hole from the other
  side.
- **Let the shell retire clips whose source disappears.** Rejected: the material *is* replaced,
  so a clip naming a source the pool no longer holds should read as a missing source — the
  arranger's own error. Keeping the stale file addressable preserves the wrong audio instead.
- **Keep `{id}.ch0` and re-point it at the new mono file** so clips on it survive. Rejected: it
  invents a second addressable source for one file, which is the per-channel convention's whole
  point (one source per channel, `{id}.ch{k}` meaning "channel k of `{id}`"). A mono file has no
  channel 1; pretending otherwise lies about the material a clip would load.

## Consequences

- Importing a mono file over a stem a split left as `{id}.ch0`, `{id}.ch1` leaves exactly one
  source, `{id}`, holding the new material; both siblings and their `.peaks` sidecars go. A clip
  on `jam.ch0` resolves to nothing instead of to the replaced take's left channel.
- A clip naming `{id}` itself is unaffected — the mono arm wrote `{id}.wav` before and still
  does. The multi-channel paths are untouched: `a_stereo_file_imports_as_two_mono_sources`,
  `a_stereo_import_replaces_the_mono_source_holding_that_id` and
  `a_narrower_import_removes_the_older_channels` all pass unchanged; none of them pinned the
  buggy behaviour, and none needed updating.
- One regression test,
  `a_mono_import_removes_the_older_split_channels` (`crates/media/tests/pool.rs`), covering the
  reverse order the report found uncovered: it imports the stereo file, then a mono `jam.wav`
  over the same stem, and asserts the index holds only `jam` with the new samples and that
  neither sibling survives. Verified to fail on the unfixed call — the index is
  `["jam.ch0", "jam"]`.
- Nothing on the render path moved: this is a pool-directory sweep at import time, bounded by the
  directory's own entries and before any audio is read.
- The stereo-pool note's `replace_sources` rule is updated to state the mono arm's count.
- `cargo test -p media` green (163 tests, 7 pre-existing hardware/soak ignores); `cargo build
  --workspace` clean.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
