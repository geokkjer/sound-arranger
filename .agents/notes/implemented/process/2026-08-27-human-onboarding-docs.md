# Agent Note: Human-onboarding docs — reading order, freshness banners, first-session tour

Status: implemented

## Problem

The docs explaining the code to the project's one human (a novice Rust developer) were good but not serving him: nothing routed a newcomer through them in working order (the top-level README's documentation map listed 468 lines of locked-decision research before any entry point), the architecture explainer had already drifted within days of being written (three crates vs. four; no mention of `flush_scheduled`; a stale criticism of since-fixed README text; a moved decision-note path), and despite a repo full of explanation there was no path from zero to *operating* the thing — which is the fastest confidence-builder for the human in the loop. At agent velocity, drift compounds faster than it does in human-paced repos, and nothing warned a reader how fresh a document was.

## Decision

Three changes, same commit:

1. **Reading order is explicit and front-loaded.** The README's Documentation map now leads with an ordered path for a newcomer: operate something ([`docs/FIRST_SESSION.md`](../../../../docs/FIRST_SESSION.md)) → learn Rust through the codebase ([`docs/rust-course/`](../../../../docs/rust-course/README.md)) → learn why ([`docs/architecture-explainer.md`](../../../../docs/architecture-explainer.md)) → only then RESEARCH.md and the notes. The course README and explainer cross-link accordingly.
2. **Freshness banners.** Explainer docs carry a banner line — `Last verified against commit <hash> (<date>). If the code has moved on, trust the code and move this line forward.` — added or advanced *when a doc is next actually touched or re-verified*, never retroactively (a banner claims verification that didn't happen). AGENTS.md carries the standing order.
3. **FIRST_SESSION.md** — a verified-by-execution fifteen-minute tour: assemble the four-plugin profile (euclidean → scale → tone → mixer) as a host-CLI script, bounce a WAV, read the summary invariants, prove byte-identical determinism with `cmp`, break it three ways on purpose (parse-time refusal exit 2; use-before-mount refusal exit 1 with refused-not-logged semantics; the units trap where `note_len=0.25` truncates to zero frames and bounces silence). Every command in it was run before being written down.

## Alternatives considered

- **Banner every existing doc retroactively** — would claim verification that didn't happen (e.g. `audio-latency.md` was last checked against nixpkgs 2026-08-13, not today). Rejected: banners mean something only if they're earned.
- **Generated freshness** (bot updates banners per commit) — machinery over discipline; banners are cheap by hand and the git history already attributes each doc's era. Rejected until the count of explainer docs makes hand-maintenance real work.
- **Fold FIRST_SESSION into the course README** — different jobs: the tour is operating manual (~15 min, copy-paste), the course is ~9 lessons of study. Keeping them separate keeps each short enough to start.
- **Make the tour use recorded clips (`pool`/`arrange`) instead of synthesis** — closer to the product's end goal but requires source WAVs on disk and drags capture setup into minute one. The synth chain proves the whole contract with four mounts; clip arrangement is pointed at as a follow-on. Rejected for v1.

## Addendum (2026-09-24) — the alpha's onboarding set

The plan's item 16 ("onboarding + honesty") added three docs and moved the reading order from the
*host* to the *product*:

- [`docs/tui-first-session.md`](../../../../docs/tui-first-session.md) — the first-session tour of
  the **primary shell** (the ratatui TUI): open a WAV, get around, cut/copy/append, shape, warp to
  tempo, name markers and clips, mount the mastering chain, export a mix, keep and reopen the
  session, and break it three ways on purpose. It replaces nothing: `FIRST_SESSION.md` remains the
  tour of the **headless host** (the `host v1` language underneath), and the README now sends a
  newcomer to the terminal shell first.
- [`docs/capabilities.md`](../../../../docs/capabilities.md) — the capability/limitations statement:
  what works (with the test that proves it), the deliberate alpha cuts *with their reasons*, the known
  rough edges, the honest flaky list, and measured performance numbers.
- [`docs/beta-acceptance.md`](../../../../docs/beta-acceptance.md) — the checklist a **second person**
  runs from the docs alone: their own files, a different sample rate, an export they hand to someone
  else, plus crash recovery and byte-reproducibility. Each item says how it *fails*, and the last
  section says what to capture when it does.

Two things it changed in the code, because writing the docs found them: **save is now idempotent**
(`session_rate` is context, not an edit, so it is no longer recorded in the history — every
open-and-save cycle used to grow `session.txt` by a line: 2 → 3 → 4) and the *markers do not change
the audio* claim the checklist makes now has its own test
(`markers_do_not_change_the_exported_bytes`). The terminal path the tour describes is re-checked by
`spikes/tui-shell/scripts/pty-check.sh`, which now drives the workflow keys (marker jump, split,
copy/paste, the prefilled prompts) through a real pty.

## Consequences

- A newcomer has one unambiguous entry point; docs audibly state their own age.
- The map now also links the **product-side** explainer ([`docs/clip-arranger.md`](../../../../docs/clip-arranger.md)) and the UI design docs ([`docs/design/`](../../../../docs/design/)), both added 2026-08-27. The clip-arranger substrate was the explanation gap the v1 tour deferred (the `pool`/`arrange` recorded-clip route was pushed off in Alternatives) — it is now an explainer, not a tour.
- The banner rule adds one maintenance duty per doc-touch — accepted, small.
- The tour depends on script syntax and output wording (`host: bounced …`, exit codes); if those change, FIRST_SESSION is the canary, which is fine — it exists to be rerun.
- Side finding worth keeping: parameter lengths are frame-valued (`note_len`, `blip_len` are `u32` sample frames); the tour documents this because even its author bounced silence on the first attempt.

*Authored with GLM-5.3 Flash · ZCode, 2026-08-27.*
