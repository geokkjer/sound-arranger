# Agent Note: GLM-5.3 review fixes — determinism, pan reachability, refused-mount state, and nits

Status: implemented

## Problem

The 2026-09-05 GLM-5.3 value-pass review (archived under
`research/architecture/2026-09-05-glm53-review-stereo-bridge-chop.md`) found
five blocking issues. Three were re-confirmed against code and are fixed here;
the two that touch the recorder/arranger underrun paths are deferred (below).

## Decision

- **#1 — determinism across a mid-call master-width change.** A logged mixer
  Mount/Unmount could change the bus width *inside* a single render call, so
  `render_block`'s frame count split and a replayed one-call render advanced the
  clock wrong (live 2048 vs replay 1536) — breaking byte-identical replay.
  Fixed in `crates/engine/src/render.rs`: `changes_master_width` parks a mixer
  Mount/Unmount at the render-call boundary (like arrangement ops), so the width
  is constant per call and the frame count stays exact. Regression test
  `replay_across_mixer_mount_in_one_call_advances_the_clock_exactly`.
- **#2 — pan unreachable from the Host API.** `HOST_PARAMS` (the closed param
  vocabulary the text format validates `set_param` against) never gained
  `ch{i}.pan`; the mixer's `MIXER_PARAMS` did. Added `ch0..ch7.pan` to
  `HOST_PARAMS` (`crates/host/src/lib.rs`).
- **#4 — a refused mixer mount mutates session state.** `process` set
  `mixer_channels` before `apply`, so a refused re-mount overwrote the live
  channel count. Now the count is committed only after `apply` succeeds.
- **Nits.** `ChopClip` now preserves the clip's outer fades on the first/last
  piece (instead of silently zeroing all fades); `Graph::connect` refuses an
  audio cord whose ports' channel counts differ (a stereo→mono cord would
  silently sum interleaved as mono); the host bounce budget accounts for a
  stereo master (up to `frames * 2 * 4` bytes).

## Consequences

- The three confirmed bugs are closed: `set_param mixer ch0.pan …` works from the
  text format, a refused mixer mount leaves state untouched, and replayed
  one-call renders stay frame-exact across a mid-session mixer mount.
- `cargo test --workspace` is green.

## Deferred (documented, not fixed here)

- **#3 — `play` after any render breaks the session** (the player node is
  appended to the topological order and so lands *after* the mixer; a cord
  player→mixer then violates forward-order). The recorder/player path needs the
  same `insert_before(mixer)` treatment `wire_arranger` uses. Not on the
  clip-arranger core path; a follow-up.
- **#5 — arranger underruns are counted but never surfaced.** The per-track
  `ArrangerNode` underrun counters go into the graph and nothing reads them, so a
  reader slip silently glitches a bounce at the host boundary. Needs threading
  the counters out of the (re-wired) nodes. A follow-up.

## Alternatives considered

- **Re-read `out_channels()` per chunk in `render_block`** (the review's first
  suggestion): still can't represent two widths in one fixed-size buffer, so it
  would require the caller to size for the change. Parking at the boundary is
  simpler and matches the existing arrangement-op parking. Chosen.
- **Special-case only "mixer"** in `changes_master_width`: the mixer is the only
  stereo master today, so the heuristic is exact for the current design; a future
  second stereo owner would extend the predicate. Accepted, documented.

*Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-05.*
