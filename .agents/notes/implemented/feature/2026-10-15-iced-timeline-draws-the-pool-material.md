# Agent Note: the iced timeline draws the pool material, and times its own frame

Status: implemented

## Problem

The iced shell's timeline drew rectangles: one lane per track, each clip a bar mapped from
`at_frame`/`src_len`, a playhead — and nothing inside a clip. Acceptance criterion 2 of the
[iced evaluation](../../proposed/architecture/2026-09-21-iced-shell-evaluation.md) is the measurement that
actually decides the canvas: *"draws the real media-pool peaks for N clips with the same viewport
maths as the Vue path (zoom/pan/seek), and holds frame rate while the transport runs."* Three things
stood between the spike and that measurement.

**A shell could not reach a clip's material.** The snapshot published the pool's source **ids**
(`pool_ids`) and the pool's **directory** was session-private, so a shell could name a source but not
open it. The `.peaks` sidecars were writer-only in practice: `PeakFile::read` returned a tuple
`(base_bin, levels, frames, rate, data)` with no query on it, and the only envelope query in the tree
was `PeakBuilder::range_minmax` — which walks one bin per 256 frames, so drawing a whole piece by
calling it per column would walk the whole source **every frame**. The naming of a sidecar
(`wav.with_extension("peaks")`) lived in three places (capture, pool, and nothing else).

**There was nothing to draw.** The demo profile is a euclidean → scale → tone → mixer chain with no
clips, so the canvas was a lane bed on every default run, and no run could show a waveform.

**The workflow's `+ - 0` had no timeline meaning in iced.** The shared table already models the split
— `Action::Zoom`/`Action::Fit` are *"the focused panel's more/less: zoom the timeline, or ride the
fader"*, and `Action::CycleFocus` exists on `Tab` — but the iced shell had no focus concept, so it
spent `Zoom` on the fader always.

## Decision

**The shell reads the persisted pyramid through the pool, the canvas draws it with a viewport, and
the shell measures its own frame.**

1. **`media::peaks` gains its read side** (`Peaks`, `PeakFile::load`): the parsed `.peaks` with
   `frames()`, `base_bin()`, `bin_frames(level)`, `level_for(frames_per_column)` and
   `range_minmax_at(level, start, end)`. The level is chosen **from the column width**, so a column
   reads one or two bins at any zoom instead of `frames_per_column` frames — the whole reason the
   pyramid has levels. `PeakFile::read` stays the raw parser; `load` is the same data in the shape a
   drawing wants. Queries at the edges (`start >= end`, `end > frames`, an empty coarse level,
   a truncated sidecar) return `None`, never a panic and never a fabricated sample.
2. **`PeakFile::sidecar_path(&wav)` is the one home of the sidecar naming.** `media::Capture`,
   `Pool::list` and the shell all call it, so the writer, the index and the reader cannot disagree
   about where a source's peaks are.
3. **The pool publishes its directory, not a listing and not a listing's data.**
   `Snapshot::pool_dir` (and `HostSession::pool_dir()`) carries the directory the session bound.
   A shell resolves a clip's `source` through **the pool's own guarded lookup**
   (`Pool::path_for`, which refuses a crafted id — a clip's source is a token in the log) and reads
   the sidecar beside the file. Envelope *values* are deliberately not published: the useful range
   depends on the zoom, the value is per-clip and per-column, and it is cheap exactly because it is
   read once from the file the pool already wrote.
4. **The iced shell caches pyramids, keyed by source id**, loaded on the poll tick and dropped when
   the pool directory changes (a rebind is a different pool). A failure is cached too, so a missing
   sidecar cannot become a file read per frame, and it is **shown**: a clip without peaks is drawn as
   a muted bar — never as silence — counted in the timeline caption, and its reason is said once in
   the status line.
5. **`--wave <file>` imports a piece into the spike's own session pool** (one source per channel at
   the session rate, each cut into three **consecutive** clips) and loads the script that places it.
   Consecutive so the piece plays like the file; cut up so the canvas has clips whose `src_start` is
   not 0 — the case a waveform mapping gets wrong by drawing the pyramid from frame 0. The pool is
   the spike's own `target/iced-pool`, the same one `--record-check` records into.
6. **The timeline gets a viewport, in points.** `Viewport` (start, frames per point, the width the
   last draw fitted to) zooms by two around the playhead, scrolls an eighth of a screen, fits, clamps
   inside the piece, and refuses to zoom below **one base bin per point** — below that the envelope
   would repeat a bin and claim detail the sidecar does not hold. It is **shared with the canvas
   program** behind a mutex: the draw is the only code that knows the plot width, and the keys read
   that width back.
7. **Focus, not new keys.** `Tab` moves the focus between console and timeline; `Zoom`/`Fit` mean
   zoom/fit when the timeline has it and ride/unity when the console does; `h`/`l` scroll;
   `j`/`k` move the console strip or the active track; a **click in the plot seeks** through the same
   `TransportSeek` a key sends (snapped to the workflow's grid when one is armed). No key was added
   to the shared table — the workflow already said what these keys mean on a focused panel.
8. **The clip's frames come from one pure mapping.** `clip_source_ranges(clip, from, to)` returns the
   source ranges a timeline range reads, honouring the region offset (`src_start`), a reversed region
   (a mirrored *range*: a min/max envelope does not care about order) and a baked loop (one range
   inside a loop, two across a boundary, the loop itself when the column covers a whole loop or more).
   `envelope_columns` then queries the pyramid once per point column, at the level the zoom implies.
   Both are free of the renderer and unit-tested.
9. **`--perf` measures the loop, not the intent.** It loads the piece, plays it, samples the redraw
   cadence for six seconds, and prints the frame percentiles, the **stalls by value**, the envelope
   columns in the last redraw, and what the shell held. The first 30 intervals are reported
   separately as startup (window + shader warm-up) rather than averaged into the frame budget, and
   the tool says out loud what it cannot attribute: a window the compositor is not showing receives
   no frame callbacks, so a gap between redraws can be the compositor rather than the shell. A
   control run with zero envelope columns is the way to tell them apart.

## Alternatives considered

- **Publish the envelope (or the pyramid) on the snapshot.** Rejected: the useful range depends on
  each shell's zoom, so the host would publish data at a resolution it cannot know; it is real bytes
  per clip per tick; and the sidecar the pool already writes is exactly this value, on disk, for the
  UI's asking.
- **Publish an `Arc<media::Pool>` instead of the directory.** A close second — it would hand the shell
  the guarded lookup directly. Rejected for the smaller surface: the session already holds the
  directory, `Pool::open` is a cheap validation, and a snapshot that clones a `PathBuf` per tick adds
  less than one that clones a handle.
- **Let the shell join `{id}.peaks` onto the directory itself.** Rejected: it is the same convention
  in a second home, and the join is exactly what must *not* happen to a source id (`Pool::path_for`
  is the guard).
- **Draw the envelope by rebuilding the pyramid from the WAV**, which is what the TUI's
  `Source::load` does today. Rejected: it reads every sample to draw a picture of data that is
  already persisted. Switching the TUI to `PeakFile::load` is the obvious follow-up; it is not in
  this slice because it changes the TUI's panel and its tests, and this slice's job was the
  measurement.
- **Query at the base level per column.** Rejected: `range_minmax` walks one bin per 256 frames, so a
  fitted whole piece would walk the source once per frame — the cost the levels exist to avoid.
- **Cache the canvas geometry (`canvas::Cache`).** Rejected again, for the same reason as the earlier
  timeline slice: the arrangement can change on any frame (an edit, an undo, a load) and the shell
  has no cheap change signal, so a stale cache would draw a piece the session no longer holds. The
  pyramids are the cache that matters (I/O, not geometry).
- **Lift the viewport into the `workflow` crate.** Rejected: the crate's own doc says what is not
  there — *"geometry and arguments … a shell owns the playhead, the active track, and the text it
  builds"*. The keys and their per-panel *meanings* are shared; the units are not (the TUI's viewport
  is terminal cells, with braille sub-columns).
- **Add `TimelineZoom`/`TimelineScroll` actions to the workflow.** Rejected: `CycleFocus`, `Zoom`,
  `Fit` and `Timeline` already exist and already document the per-panel reading. Adding keys would
  have made one shell's gap into the table's vocabulary.
- **Draw a clip without peaks as a plain bar.** Rejected: a silent-looking bar is the read that "this
  clip is silent". The muted colour, the caption count and the one status line cost nothing and are
  true.
- **Measure headlessly** (drive the draw maths and time it) instead of opening the window. Partly
  done — the wave test covers import → pool → sidecar → envelope end to end, headless — but the
  criterion is frame rate *while the transport runs*, and that needs the real loop, so `--perf` is a
  windowed, owner-run instrument like `--sweep` and `--record-check`.

## Consequences

- **Criterion 2 is answered for peaks, viewport and frame rate.** Real envelopes from the persisted
  pyramid, per-shell viewport maths with the TUI as the reference, click-to-seek, and a measured
  loop: on this host, with a 30 s stereo piece (2 tracks, 6 clips, **1,628** envelope columns per
  redraw) under Mesa's **software** renderer, **steady p50 16.7–17.0 ms (59–60 fps), p95
  18.7–30.4 ms** across three runs, with the transport running throughout. The demo profile (no
  clips, 0 columns) measures p50 13.3 ms (75 fps) in a clean run, so the envelope drawing costs a few
  milliseconds inside a display-rate frame. Some runs show redraw **gaps** of 0.4–1.0 s; they appear
  in a run with 0 envelope columns as well, so they point at the compositor (a window it is not
  showing receives no frame callbacks) rather than the draw — and the instrument reports them by
  value, with that caveat, instead of averaging them into a rate.
- **Clip editing is still unbuilt, and still says so.** `x d < > t H L J K g G f F v n N` report the
  editing gap in the status line. The canvas now exists, so the messages name *editing*, which is
  what is actually missing (the previous slice made the same move for the canvas).
- **The snapshot gained one field, and a shell can reach material.** `Snapshot::pool_dir` is the
  session's bound directory; `HostSession::pool_dir()` is its getter. The host's test
  (`the_pool_id_cache_lists_the_pool_and_survives_a_failed_refresh`) asserts it is `None` before a
  bind and the bound directory after.
- **`PeakFile::sidecar_path` moved the naming into one home.** `media::Capture::finish` and
  `Pool::list` now call it; a future writer cannot invent a third spelling.
- **`media::Peaks` is a queryable read side, pinned against the write side.** A test loads a sidecar
  and asserts the base-level answer is **bit-identical** to the finalized builder's, that a coarse
  level contains it, that the level choice gives at most a couple of bins per column, and that the
  edges are `None` rather than a panic.
- **The TUI still rebuilds pyramids from the WAV.** Its timeline draws from a `PeakBuilder` fed by
  every sample of each source. The read side now exists to remove that; it is a follow-up slice with
  its own risk (the TUI's panel and its envelope tests).
- **The viewport is a near-duplicate of the TUI's maths, in different units.** Accepted and written
  down: the laws are the same, the unit is not, and the workflow crate deliberately owns neither.
  The duplication is the cost of the per-shell unit; the reference is named in both files.
- **`--wave` shares `target/iced-pool` with `--record-check`**, so the pool can hold a recorded take
  and an imported piece at once. The wave test removes its own fixtures, so a test run does not grow
  the pool.
- **Tests: 29 in the spike** (8 new: viewport mapping, the pyramid floor, a degenerate view, the
  region/reverse/loop mappings, the envelope columns, and `--wave` end to end), 4 new in `media`, and
  the host assertion. `--perf` itself cannot be a test (it needs a display) and is the owner's
  instrument, documented in the spike README.

Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-10-15.
