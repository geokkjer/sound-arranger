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
is the last node. A naively re-added arranger node lands *after* the mixer and cannot connect.
Channels also map to track **index**, so removing a track re-numbers its successors —
incremental patching can't express that.

## Decision

Wire the arranger by **reconcile-on-dirty**:

1. **`Graph::insert_before(pivot, kind, ports)`** — a new graph primitive that inserts a node
   immediately before `pivot`, shifting `pivot` and all later nodes (and their cord indices) up
   by one. This is the one way to add a node that must precede an existing node while satisfying
   the forward-order rule, and it does not steal the master-bus claim.
2. **`wire_arranger` reconciles** on each dirty render. It **validates before
   mutating** (the engine's `OpHandler` contract: an `Err` means nothing changed):
   first it materializes the mixer and resolves it (via `engine.node_of("mixer")`,
   not `graph.out_node`), then builds every new `ArrangerNode` (the fallible part —
   missing source, rate mismatch, too many tracks). Only when both succeed does it
   drain `wired_tracks`, `remove_node` every old node, then `insert_before(mixer)`
   each new node and connect to `ch{track_index}` — the commit phase cannot fail
   (live pivot, `ch0..ch7` declared). `arrange_dirty` is cleared only on success.
3. **Readers are positioned at the transport frame.** `ArrangerNode::new` takes `from_frame`; each
   reader is built via `FilePlayer::start_looped_anchored(clip, loop_len, cap, off0)` where
   `off0 = from_frame.saturating_sub(clip.at_frame).min(clip.src_len)` — the clip offset the
   transport is currently at. The reader **continues** the clip (and a looped clip stays in phase,
   wrapping back to its true region start) rather than restarting it from source[0]. The render
   alignment assert is `popped + off0 == off`, where `off = fr - at_frame`.
4. **The mixer is resolved by plugin name** (`engine.node_of("mixer")`), not by `graph.out_node` —
   the bus points at whatever audio node was mounted last, so `out_node` is a misleading mixer
   proxy when the mixer has been unmounted.

Reconcile fixes both failures at once: a removed track is not rebuilt (so its audio stops), and an
edited track is rebuilt from the current value (so the edit reaches audio). The rebuild is a
deterministic function of the value, so byte-identical replay holds *and the rewire path is
exercised and asserted* by a bounce → edit → re-bounce replay test (the pre-existing single-bounce
replay tests never ran the rewire path).

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
- **Rebuild at source[0] (the initial bug)** — rejected after a co-worker (GLM-5.3) review found
  it was a **critical restart bug**: a node rebuilt at the current transport frame restarted every
  clip from its source start (`popped=0` while `off` was already `off0`), playing the wrong region
  and tripping the `debug_assert` for clips longer than the ring's 64 Ki frames. The fix is step 3
  (thread `from_frame` and position the reader). My first tests passed *vacously* because they used
  a constant-valued source; the review called this out, and the fix adds a ramp-source test that
  asserts the correct source region plays.

## Consequences

- `RemoveTrack` no longer ghosts: a removed track's node is retired on the next render, and the
  second render's frames are silent. Verified by a test that renders a long clip, removes the
  track, and asserts the next render is silent (the old add-only code returned non-silent frames).
- An edit to a wired track reaches audio *and continues the clip from the transport position*, not
  from its source start. Verified by a ramp-source test asserting the second render plays the
  source region after the first render (a restart would play the clip's beginning). The looped-clip
  rebuild is covered too — a node-level test rebuilds a looped clip mid-cycle and asserts it stays
  in phase (`pattern[(50+k)%100]`, not `pattern[k]`).
- The graph gained `insert_before` (2 tests: precedence before a sink, cord reindexing) and the
  engine gained a read-only `node_of` accessor (a true-mixer lookup). The anchored reader is
  covered directly (media `stream.rs`): a non-looped reader continues from `off0`, and a looped
  reader wraps to its true region start. Host suite gained a bounce → edit → re-bounce byte-identical
  replay test. 147 tests across the workspace (was 139 before this work), clippy clean.
- **Known edge**: a clip declared longer than its source file (`src_start + src_len` past the file's
  last frame) renders truncated-but-fine from frame 0, but a rebuild past the real EOF makes the
  reader's `seek_frames(start + phase)` fail loud — a consistent, loud error rather than silent
  wrong audio. (Handling it as "clamp + silence" would need a deliberate choice; left for the
  dynamic-edit work rather than bolted on.)
- **Deferred**: the shared-state reader-reuse reconcile (the dynamic-edit architecture); an edit
  *while playing* still rebuilds readers (warm-up on every edit — acceptable for the reference
  host's build-then-bounce, too heavy for live playback).
