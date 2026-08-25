# Agent Note: arranger reconcile-on-dirty — the RemoveTrack ghost + edits reach audio

Status: implemented

## Problem

The reference host wired the arranger **add-only**. `wire_arranger` iterated the current
timeline and mounted a fresh `ArrangerNode` only for tracks not already in `wired_tracks`; it
never retired or rebuilt a wired node. Because `ArrangerNode` holds a **snapshot** of its
track's clips, any edit after the first wiring was invisible to audio. Two concrete failures:

- **`RemoveTrack` ghost-played.** Removing a track dropped it from the `Timeline` value, so the
  host's add-only loop no longer saw it — but its node stayed mounted and kept playing. The UI
  showed no track while audio continued. Worse than "an edit is silent": the host actively lied.
- **Edits to a wired track were ignored** (the P1.3.4 note's "an edit after a bounce is silent").
  A `SetClipGain`/`MoveClip`/`Trim` mutated the value but not the mounted node's snapshot.

The structural blocker: `Graph::connect` requires **forward order** (`from < to`), and the mixer
is the LAST node (it claims `out_node`). A naively re-added arranger node lands *after* the
mixer and cannot connect. Channels also map to track **index**, so removing a track re-numbers
its successors — incremental patching can't express that.

## Decision

Wire the arranger by **reconcile-on-dirty**:

1. **`Graph::insert_before(pivot, kind, ports)`** — a new graph primitive that inserts a node
   immediately before `pivot`, shifting `pivot` and all later nodes (and their cord indices) up
   by one. This is the one way to add a node that must precede an existing node while satisfying
   the forward-order rule, and it does not steal the master-bus claim (falls back to `add_node`'s
   `out_node`-only-if-`None` rule).
2. **`wire_arranger` reconciles** on each dirty render: drain `wired_tracks`, `remove_node` every
   old arranger node, `flush_scheduled` (materialize the mixer as the insert pivot), then for each
   current track build a fresh `ArrangerNode` and `insert_before(mixer)` it, connecting to
   `ch{track_index}`. Each track always maps to its index, so a removal also re-numbers later
   tracks correctly.

Reconcile fixes both failures at once: a removed track is not rebuilt (so its audio stops), and an
edited track is rebuilt from the current value (so the edit reaches audio). Byte-identical replay
still holds — the rebuild is a deterministic function of the value.

## Alternatives considered

- **Retire-only** (drop nodes whose track vanished, keep the rest) — rejected: it fixes the ghost
  but not edits, and it breaks the channel-by-index invariant (removing a track would leave the
  later track on a stale channel).
- **In-place reader reuse** (the shared-state `ArrangerNode`, the P1.3.4 note's deferred
  dynamic-edit architecture) — the *real* long-term fix for live edit-while-playing, where
  re-warming readers on every edit is too heavy. Deferred: rebuild-on-dirty is correct and
  deterministic; reuse needs a reader-reconcile path that SPSC can't reposition mid-clip.
- **Mixer-remount dance** (unmount → rebuild arrangers → remount → re-apply mixer params) —
  rejected: heavyweight and would have to re-apply every mixer param; `insert_before` is simpler.

## Consequences

- `RemoveTrack` no longer ghosts: a removed track's node is retired on the next render, and the
  second render's frames are silent. Verified by a test that renders a long clip, removes the
  track, and asserts the next render is silent (the old add-only code returned non-silent frames).
- An edit to a wired track reaches audio: a `SetClipGain` after a render is rebuilt into the next
  render's amplitude. Verified by a control/vs-raised test.
- The graph gained `insert_before` (2 tests): precedence before a sink and cord-index reindexing
  after the insertion point. 143 tests across the workspace (was 139), clippy clean.
- **Deferred**: the shared-state reader-reuse reconcile (the dynamic-edit architecture); an edit
  *while playing* still rebuilds readers (warm-up on every edit — acceptable for the reference
  host's build-then-bounce, too heavy for live playback).
