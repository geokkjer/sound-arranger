# Agent Note: frontend unit tests — vitest for the pure viewport/ruler logic

Status: implemented

## Problem

The Vue shell had **no tests at all**, while the Rust side is heavily tested. That gap started to
matter with the timeline viewport: `zoomAt` (cursor-anchored zoom), `clampView` (the fit floor, the
px/s ceiling, `t0` bounds) and `followPlayhead` (where the view parks) are **pure, regression-prone
logic** that neither `tsc` nor a compile can catch — and a subtly wrong anchor or clamp is invisible
until it is annoying in the hand. The ruler's tick step/label logic sat inside the canvas SFC, out of
reach of any test.

## Decision

- **Vitest** as the frontend test runner (`devDependencies`, pinned `^3` for the Vite 6 peer), with
  `pnpm test` (`vitest run`) and `pnpm test:watch`. It reuses the existing `vite.config.ts`, so there
  is **no separate vitest config** and no second transform pipeline.
- **Tests are colocated** (`src/*.test.ts`) and run in the default **node** environment — the pure
  maths needs no DOM.
- **The ruler maths moved out of the SFC** into `src/timelineTicks.ts` (`tickStepFrames`,
  `tickLabel`, `MIN_TICK_PX`, `NICE_SECONDS`) so it is testable; the canvas now imports it. The
  viewport state + maths already lived in `src/timelineView.ts` (a reactive singleton + pure
  functions), and the tests reset that singleton per case.
- **Coverage is the viewport's real risk**: anchored-zoom invariance, the fit floor and the ceiling
  (including "fit wins when fit is above the ceiling"), `t0` clamping at both ends, pan, follow
  parking/recentring, the tick step picking/fallback, and the label formats. 23 tests, two files.

## Alternatives considered

- **No frontend tests** (the status quo). Rejected: the viewport is genuine logic and the Rust side
  sets a testing expectation the frontend was not meeting.
- **Jest.** Rejected: a second transform/config pipeline for ESM+TS+`.vue` on top of Vite; vitest
  reuses the Vite config and the same module graph.
- **Component/DOM tests now** (`@vue/test-utils` + happy-dom/jsdom). Deferred, not rejected: the risk
  concentrated in the pure maths, and a DOM environment plus a bigger dependency surface is not
  earned yet. Add it when a component's behaviour (not its maths) needs pinning.
- **Move the maths into its own package** to test it in isolation. Rejected: a single frontend
  package; the module boundary inside `src/` already gives the separation.

## Consequences

- `cd crates/shell && pnpm test` runs the suite; `pnpm typecheck` type-checks the tests too (they are
  under `src/`, so `vue-tsc` sees them), and `pnpm build` is unaffected (tests are not reachable from
  the entry, so they are not bundled).
- CI (when there is one) can run `pnpm test` alongside `cargo test`.
- The canvas SFC got thinner (tick logic extracted); new pure logic should land in a testable module
  rather than inside the SFC.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-10.
