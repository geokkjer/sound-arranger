# Agent Note: the inline regression tests leave the source files — a future split, recorded now

Status: proposed

## Problem

The space-bunny fix run ([disposition](../../../../research/architecture/2026-09-29-space-bunny-review.md))
landed 26 defects' worth of fixes plus a regression test per defect, and the suite grew from 326 to
416 tests. That is exactly the weight a bugfix run should add — but most of it landed **inline**, and
the raw file sizes now mislead anyone who opens them:

| file | total lines | production | inline `#[cfg(test)]` |
|---|---|---|---|
| `crates/host/src/lib.rs` | 10,379 | 3,052 | **7,327 (71%)** |
| `crates/media/src/timeline.rs` | 2,924 | 1,098 | 1,826 |
| `crates/engine/src/graph.rs` | 2,448 | 1,724 | 724 |
| `crates/media/src/wav.rs` | 1,250 | 592 | 658 |
| `crates/engine/src/plugins/clock_out.rs` | 945 | 524 | 421 |

(Measured at `ea01d0d`, the branch tip the merge review gated; `engine/src/render.rs` is the
counter-example — 1,931 lines, all production, +63% in one run.)

`host/src/lib.rs` is the acute case. Its 3,052 production lines already do six jobs — the
`HostCommand` enum and dispatch (~340), `HostSession` core including a 444-line `apply` match
(~1,960), history/replay/undo (~630), journal persistence (~345), script formatting (~330), and
script parsing (~815) — and then 7,327 lines of tests sit on top in two modules (`mod tests` alone
is ~5,000 lines). The costs are concrete: navigation (the file's real structure is invisible until
line 4,821), recompile latency (every touch of the crate recompiles the whole inline suite), and
review size (a fix near the top produces a diff in a 10k-line file).

## Proposal

A future decision, made now so the next pass does not have to rediscover the numbers. Three ordered
slices, tests first because it is the mechanical, risk-free half:

1. **`slice/test-mods-out-of-source`** — the regression tests move into their own files. The inline
   mods become `#[cfg(test)] mod tests;` declarations pointing at `src/tests/*.rs` submodules:
   still unit tests, still private-item access, zero behavior change. `host` first (the two mods in
   `lib.rs`), then the other files where the inline suite outweighs the code it tests
   (`timeline.rs`, `wav.rs`, `clock_out.rs`). Standing rule from then on: **an inline test mod that
   outgrows the code it tests moves out in the same change that made it outgrow.**
2. **`slice/host-module-split`** — once the tests have moved and the file is ~3,000 lines, extract
   the seams that already exist as comment-level boundaries: `command.rs` (the enum and dispatch),
   `history.rs` (rebuild/replay/undo/redo), `journal.rs` (entry types and read/write helpers),
   `script/parse.rs` + `script/format.rs`. `live.rs`, `media_ops.rs`, and `rig.rs` are the
   precedent — the crate already knows how to do this.
3. **`slice/engine-render-split`** — `render.rs` is the next god-module candidate, not the current
   one: one `Engine` struct carrying mount validation, patch validation, fault recording, replay,
   scheduling, and rendering, all production. Split the validation and `ApplyFault` machinery into
   engine-internal modules before the next growth spurt, not after.

## Alternatives considered

- **Move the tests to `crates/*/tests/` (integration tests).** Rejected: they exercise private
  items — `parse_script`, the `apply` internals, journal helpers — so integration placement would
  either force `pub` surface that should not exist or silently drop coverage. In-file submodules
  (`src/tests/*.rs`) keep both.
- **Do nothing; the sizes are mostly tests, which is healthy.** Rejected: the growth rate is the
  problem, not the ratio. The fix run added ~2,100 test lines to `host/src/lib.rs` in one pass; the
  next review-sized run starts from 12k.
- **Fold the refactor into the fix branch.** Rejected for this merge: the branch's value is its
  per-commit adversarial verification, and a mechanical mega-diff mixed in dilutes both the
  verification story and the blame chain. The human confirmed the separation at the merge review.
- **Split the production modules first, move tests later.** Rejected ordering: moving tests is
  mechanical and provable (suite count and results unchanged), shrinks every subsequent diff, and
  the seams are far easier to see in a 3,000-line file than a 10,000-line one.

## Acceptance criteria

1. **Slice 1 proves itself by arithmetic**: after the move, the workspace suite reports the same
   test count and results as before it (416 passed, 0 failed at `ea01d0d`), the diffs contain no
   visibility or behavior changes, and `host/src/lib.rs` shrinks to ~3,100 lines — its production
   code plus the `#[cfg(test)] mod` declarations.
2. **Slice 2 lands one seam per commit** — `command`, `history`, `journal`, `script` — each against
   the full gate set (fmt, clippy `-D warnings`, workspace suite, notes verifier), with no growth of
   the `pub` surface beyond what `pub(crate)` reaches.
3. **Slice 3 leaves `render.rs` under ~1,200 lines** with the mount/patch validation and
   `ApplyFault` recording in engine-internal modules, gate set green.
4. **The standing rule is enforced from then on**: a change whose inline test mod outgrows the code
   it tests moves the mod in the same change, and a review that lets it slide cites this note.
5. **This note graduates to `implemented/` in the change that lands the last slice**, restating
   shipped reality (paths, sizes, rule) rather than intent.

## Risks

- **A test move is a large diff with zero semantic change**, which invites under-review. The gate
  for slice 1 is strict: same test count, same results, no visibility changes — anything else in
  the diff disqualifies the commit.
- **Slice 2 needs `pub(crate)` adjustments**, which is where a mechanical split quietly becomes a
  design change; each extraction should be its own commit against the same gate set.
- **Churn collides with concurrent slices.** These three land in a quiet window, not alongside
  feature work — the worktree discipline keeps checkouts separate but not review bandwidth.

## Attribution

Authored with GLM-5.3 · OpenCode, 2026-09-30, at the merge review of `fix/space-bunny-review`; the
measurements are from `ea01d0d`. The direction is the human's — the refactor is recorded as a future
decision, deliberately not actioned in the fix run's merge.
