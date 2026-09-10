# AGENTS.md

sound-arranger (working title) — an audio platform built on a minimal core (clock · graph interpreter · session log · context plumbing) with every capability as a plugin; the product is an assembled profile. First profile: the clip-arranger ("sound-arranger") — record long live jams, then cut, paste, rearrange, and shape them into a finished piece. Offline processing is a separate deferred profile ("sound sculptor"). Rust audio engine + Tauri v2 + Vue 3. Working research lives in [RESEARCH.md](RESEARCH.md); decision records live in [.agents/notes/](.agents/notes/README.md).

## Separation of concerns

**sound-arranger is software development only.** Personal, gear, and hardware content does
not belong here. The four zones and where each kind of thing lives:

- **music-theory** (analyses + aesthetic) → `~/Projects/music/music-composition-theory` (`theory/`, `analyses/`).
- **studio** (gear, catalog, hardware exploration) → `~/Projects/music/music-composition-theory/studio/instruments/<name>/research-note.md`.
- **sound-arranger** (software: engine, media, host, shell, plugin sidecars) → **this repo**.
- **other-software** (VCV Rack, Tidal, Csound, CDP, …) → their own projects (`~/Projects/music/vcv-rack`, `~/Projects/tidal-lsp`, …), never as crates here.

Rules:

- **Gear/hardware content goes to the studio project, never here.** USB characterizations,
  gear research notes, firmware images, rack build plans, and gear-specific capture scripts
  belong in `music-composition-theory/studio/instruments/<name>/`.
- **Reference, don't copy.** When a doc here needs gear/hardware context, link to the
  studio project (`../music/music-composition-theory/studio/...`) rather than storing a copy.
- **No personal/creative content in this repo** — gear, catalog, or hardware-learning notes.
  If a piece of work is about the gear rather than the software, it goes in the studio project.

## Standing orders

- **Every non-trivial change adds or updates an Agent Note in the same commit.** Mechanical or local edits are exempt. Update the note that owns the decision; supersede, never rewrite, a decision.
- **Every note carries `## Alternatives considered`** — record what was rejected and why.
- **An implemented note states shipped reality in present tense.** Keep paths, names, and defaults current in the same change that alters them.
- **`.agents/notes/archived/` is frozen.** Never edit it; the pre-commit hook enforces this.
- **`RESEARCH.md` holds working research; notes own decisions.** When a decision locks, graduate it into a note, and link from research to the note instead of restating it.
- **The pre-commit hook runs `node scripts/verify-agent-notes.mjs`.** Node must be on `PATH`; on a machine without node the hook warns and skips rather than blocking the commit.
- **External-model work is attributed** ([note](.agents/notes/implemented/process/2026-08-27-agent-attribution-convention.md)): a note/doc a model authored carries an `Authored with <model> · <harness>, <date>` footer; commits carry an `Assisted-by:` trailer. External reviews go verbatim under `research/architecture/` with a disposition header instead.
- **The co-work loop is a documented, versioned snapshot** ([note](.agents/notes/implemented/process/2026-08-27-model-co-work-routing.md)): DeepSeek-V4-Flash drives and plans, GLM-5.3-Flash is the cheap independent value pass and scoped coding handoff, DeepSeek-V4-Pro is the depth/escalation tier, and Kimi K3 gates merges. Independence (cross-vendor, not raw strength) decides the reviewer and second-opinion roles. Re-evaluate the whole table on each new model or a material price/capability shift; record the change as a new note.
- **Explainer docs carry a freshness banner** ([note](.agents/notes/implemented/process/2026-08-27-human-onboarding-docs.md)): `Last verified against commit …` — added or advanced when a doc is touched or re-verified, never retroactively.
