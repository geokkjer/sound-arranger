# Agent Note: the iced timeline draws the arrangement the snapshot publishes

Status: implemented

## Problem

The iced shell could play and record but drew a **static placeholder** where a timeline should
be ([iced-record-mvp](2026-09-30-iced-record-mvp.md), item 5). The placeholder was honest about
why: `Snapshot` carried the transport, the meters and the take state, and no arrangement, so
there was nothing to draw. The host had the value — `HostSession::arrangement()` — but it
*reconstructs* the timeline from the editor's log, so reading it on the pump was not an option:
`publish` runs on every ~4 ms tick, and a per-tick reconstruction would fold the whole `Arrange`
history once per frame, which is the cost `pool_ids` was cached to avoid.

Two things were missing, and they are one slice: a projection on the snapshot that is **cheap to
poll**, and a widget that draws it.

## Decision

**`Snapshot` publishes the arrangement, cached in the session and behind an `Arc`; the iced shell
draws it with a `canvas` program.**

1. **The value is `TimelineStatus`, and the error is modelled.** `crates/host/src/live.rs` owns
   `TimelineStatus { timeline: Option<Arc<media::Timeline>>, error: Option<String> }`, next to
   the other published state, and `Snapshot` gains a `timeline` field of it. `Arc` is what makes a
   poll cheap (a publish clones a pointer, not every clip); `error` is what lets a shell **say**
   the host could not reconstruct a poisoned arrangement instead of drawing an empty piece — the
   rule `HostOutcome::arrangement` already keeps for the one-shot response.
2. **The cache is rebuilt only where the arrangement can change.** `HostSession` keeps the
   `TimelineStatus` and a private `refresh_timeline()`; the refresh points are the end of the
   public `execute` (once per applied **state** command), `carry_over` (a seek/undo/redo adopts a
   rebuilt session), and once at the end of `from_script` (a loaded session). `new_at_with` builds
   it once, so a fresh session reports an **empty** arrangement rather than "unknown". The pump
   never rebuilds it: `publish` copies `session.timeline_status()`.
3. **`execute` refreshes; `process` does not.** `process` is also the replay path a rebuild, a
   script load and a journal walk take. Those loops call `process` and refresh **once** when they
   are done, so an undo of a long history reconstructs the timeline one time, not once per
   replayed step. `HostSession::timeline_status()` exposes the cache; `HostOutcome.arrangement` is
   untouched, because the TUI reads it and a value response is what the TUI wants.
4. **The iced shell draws the arrangement.** `Spike::timeline` builds a `canvas::Canvas` whose
   program is rebuilt from each snapshot (it owns a cloned `TimelineStatus`, which is an `Arc`
   clone): one lane per track, each clip a filled rectangle, the track id in a left gutter, and a
   playhead line at `snap.frame`. The caption carries the playhead and end in seconds and the clip
   count. When `timeline.error` is set the canvas draws **that**, never an empty timeline.
5. **The x/width mapping is one pure function.** `clip_span(at_frame, src_len, span, width)`
   (over `frame_x`) maps a clip's placement and length onto plot points, clamping a frame past the
   span to the right edge and returning zero width for a zero span. `frame_x` draws the playhead
   too, so the two cannot disagree.
6. **The span is `max(arrangement end, furthest frame played)`.** The arrangement's own end comes
   from `media::Timeline::end_frame()`; the played span is the shell's memory of the furthest
   frame the transport reached. The max is what makes the view stable in both states: a loaded
   piece shows its whole length **before** it is played, and a take recorded past the last clip
   still has somewhere to put the playhead.

## Alternatives considered

- **Reconstruct the timeline on every pump tick** (call `arrangement()` from `publish`). Rejected:
  `arrangement()` folds the editor's log and clones the value, and publish runs every ~4 ms — an
  O(piece)-per-frame cost for a shell that polls. The whole point of the cache is that the pump
  copies an `Arc`.
- **A version counter / generation number on the snapshot, with the shell re-fetching the
  arrangement through `HostOutcome` when it changed.** Rejected: it moves the reconstruction back
  onto the on-demand path and makes the shell's timeline one message behind its transport, and it
  puts the refresh policy in each shell (two shells, two policies). The host already knows when
  the arrangement changed; it should publish the value.
- **Have the shell poll `HostOutcome::arrangement` directly** (the TUI's path, and what the
  placeholder pointed at). Rejected for a shell that redraws every frame: `outcome()` is an actor
  round trip plus a fresh reconstruction, so it is per-frame work, and it leaves the one thing
  both shells poll — `Snapshot` — without the arrangement, so the canvas would need its own
  cadence.
- **Change `HostOutcome.arrangement` to `Arc<Timeline>` and read it from there.** Rejected: the
  TUI holds the value and the outcome is a one-shot command response by design; widening it does
  not make a *poll* cheap, and it would edit a shape the TUI depends on to serve the iced shell.
- **Publish a flat lane/clip list shaped for the canvas** instead of the media model. Rejected: it
  invents a second shape of the arrangement for the wire, which is the inverse of the rule that a
  shell derives its values from the engine. The media `Timeline` is the value the engine renders;
  the shell maps it at draw time.
- **Keep the static placeholder** and measure position against the played span. Rejected: it was
  honest only while the arrangement was off the wire. With the value published, a fraction bar is
  a strictly worse drawing of the same data, and it never showed a loaded piece before playing.
- **Cache the canvas geometry** (`canvas::Cache`) and clear it on change. Rejected: the
  arrangement can change on every frame (an edit, an undo, a load) and the shell has no cheap
  change signal of its own, so a stale cache would keep drawing a piece the session no longer
  holds. The canvas is rebuilt from the snapshot each redraw, and the rebuild is a pointer copy.
- **Fold the error into an empty arrangement** (publish `Some(default)` when the reconstruction
  fails). Rejected: a poisoned editor would read as "the piece is empty", the silent lie
  `HostOutcome.arrangement` refuses.

## Consequences

- **The snapshot carries the arrangement, cheaply.** `Snapshot::timeline` is a `TimelineStatus`
  whose value is an `Arc<media::Timeline>`; `publish` clones it per tick. A shell that only wants
  the transport can ignore it, and a shell that draws a timeline needs no second channel.
- **The cache's rebuild points are pinned.** A host test
  (`the_arrangement_cache_follows_the_committed_edits`) builds a pool, adds a track and a clip
  through `HostCommand::Arrange`, sees them in `timeline_status()`, and undoes the clip — asserting
  the adopted cache follows the rebuild — while an empty session reports an empty timeline with no
  error. Every existing test keeps passing, and `HostOutcome.arrangement` is unchanged.
- **The iced shell draws a real timeline**, and the placeholder is gone. `position_fraction`
  (which measured against the played span) is deleted with it; `clip_span`/`frame_x` replace it and
  are unit-tested (zero span, a clip at frame 0, a fractional mapping, a clamp past the span). The
  canvas program is built from a snapshot with no tracks, with tracks, and with an error, so the
  empty and failed states cannot panic.
- **The shell's "not built yet" messages moved to what is actually missing.** The canvas exists
  now, so a grid or editing action that still cannot quantize says the **editing gestures** are not
  built, not that the canvas is. The shared workflow is untouched: the key, the mode and the grid
  state are still the table's.
- **`spikes/iced-shell` names the media model directly**, as the TUI already does, so the shell can
  build the value its tests draw and read the fields the canvas maps. It is a sibling path
  dependency in the spike's own workspace; the core workspace is unchanged.
- **The cache is not a second source of truth.** It is the host's own reconstruction, held only to
  avoid re-deriving it per frame; two shells drawing it draw what the engine holds.

Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-10-04.
