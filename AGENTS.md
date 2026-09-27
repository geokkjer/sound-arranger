# AGENTS.md

The platform is **`audio`** (the repo rename is still pending) — built on a minimal core (clock · graph interpreter · session log · context plumbing) with every capability as a plugin, so a product is an assembled **profile**. Three profiles: **recorder** (the current focus — send a clock to external gear, capture, align, master, export), **arranger** (the ACID clip arranger — record long live jams, then cut, paste, rearrange and shape them into a finished piece), and **sculptor** (audio transformation, deferred). Rust audio engine; the shells are `ratatui` (the TUI) and `iced` — Tauri retired 2026-09-22. Working research lives in [RESEARCH.md](RESEARCH.md); decision records live in [.agents/notes/](.agents/notes/README.md); the profile decision is [here](.agents/notes/proposed/architecture/2026-09-27-profiles-and-the-umbrella-name.md).

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
- **External-model work is attributed** ([convention](.agents/notes/implemented/process/2026-08-27-agent-attribution-convention.md), [git identity](.agents/notes/implemented/process/2026-09-12-git-identity-for-agent-commits.md)): a note/doc a model authored carries an `Authored with <model> · <harness>, <date>` footer. **Agent commits go through `scripts/agent-commit`**, which sets the model git identity as **author** and leaves the human as **committer**; name the model and harness that actually ran with `--model`/`--harness` (or `$DSH_AGENT_MODEL`/`$DSH_AGENT_HARNESS`) — the email is the model's slug at `geokkjer.eu`, and `.githooks/commit-msg` appends the matching `Assisted-by: <model> · <harness>` trailer automatically. Never type the trailer by hand, and never label a commit with a model the switch was not told about: a wrong label is worse than none, because it turns an unknown into a plausible fact. External reviews go verbatim under `research/architecture/` with a disposition header instead.
- **The co-work loop is a documented, versioned snapshot** ([table](.agents/notes/implemented/process/2026-08-27-model-co-work-routing.md), [roster re-evaluation](.agents/notes/implemented/process/2026-09-27-model-roster-re-evaluation.md)): DeepSeek-V4.1-Flash drives and plans, GLM-5.3-Flash is the cheap independent value pass and scoped coding handoff, DeepSeek-V4-Pro is the depth/escalation tier, and Kimi K3 gates merges. DeepSeek-V4-Flash is retired — a retired identity is anonymous, never a route. Independence (cross-vendor, not raw strength) decides the reviewer and second-opinion roles. Re-evaluate the whole table on each new model or a material price/capability shift; record the change as a new note.
- **Explainer docs carry a freshness banner** ([note](.agents/notes/implemented/process/2026-08-27-human-onboarding-docs.md)): `Last verified against commit …` — added or advanced when a doc is touched or re-verified, never retroactively.
