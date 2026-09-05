# Agent Note: separation of concerns — personal/studio vs. software

Status: implemented

## Problem

sound-arranger's `research/` and `scripts/` had accumulated content that is really the
user's *personal* music/studio work, not the software project: hardware gear notes (USB
characterizations, the Eurorack rack build plan), the VCV Rack patch sandbox, and a
gear-specific capture script. That blurred the repo's identity and risked coupling it to
personal/catalog exploration that belongs elsewhere. The goal is for sound-arranger to be
**software development only**, and for hardware/gear/catalog exploration to happen in the
studio project.

## Decision

Enforced a four-zone separation (documented in `AGENTS.md` → "Separation of concerns"):

- **music-theory** (analyses + aesthetic) → `~/Projects/music/music-composition-theory` (`theory/`, `analyses/`).
- **studio** (gear, catalog, hardware) → `~/Projects/music/music-composition-theory/studio/instruments/<name>/research-note.md`.
- **sound-arranger** (software: engine, media, host, shell, plugin sidecars) → this repo.
- **other-software** (VCV Rack, Tidal, Csound, CDP, …) → their own projects (`~/Projects/music/vcv-rack`, `~/Projects/tidal-lsp`, …), never crates here.

Moved out of sound-arranger:

- `research/gear/` (all hardware gear notes + FM-1 firmware files) → theory `studio/instruments/<name>/`, one per instrument, as `research-note.md`.
- `scripts/capture-ep133-usb.sh` → theory `studio/instruments/ep-133-k-o-ii/` (gear tooling).
- `vcv-patch/` + `scripts/log-vcv-usage.sh` + `scripts/vcv-patch-info.sh` → its own repo `~/Projects/music/vcv-rack/` (VCV Rack learning project), `git init`'d, `.usage/` gitignored.
- `music-composition-theory/` itself moved from `~/ZCodeProject/` into `~/Projects/music/` (theory + studio now the hub for the personal set).

Repointed the references that stayed (RESEARCH.md §2, the affected agent notes, and the
moved notes' internal links) to the new locations so nothing dangles.

## Alternatives considered

- **Leave everything in sound-arranger** — keeps one repo but keeps contaminating the
  software project with personal gear/catalog content; rejected.
- **Copy (reproduce) the design notes into the studio project** — duplication is drift
  risk; the architecture notes stay in sound-arranger and are *linked* from the studio
  project instead, so there's one source of truth.
- **Symlink the old `research/gear/` / `vcv-patch/` locations back into sound-arranger** —
  avoids re-pointing references but keeps the content reachable under a software repo, which
  fights the separation; the references were re-pointed instead.

## Consequences

- sound-arranger now holds only software-dev content (engine, media, host, shell, plugin sidecars, architecture research).
- The studio/gear/catalog source of truth is the theory repo; hardware exploration happens there.
- `vcv-rack` is a standalone learning project (own repo, no longer a sound-arranger artifact).
- Cross-repo references use the `../music/...` convention (they resolve on this machine; external clones of sound-arranger won't follow them, which is already the documented rule).

*Authored with DeepSeek-V4-Flash · deepseek-harness, 2026-09-05.*
