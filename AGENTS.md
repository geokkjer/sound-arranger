# AGENTS.md

sound-arranger (working title) — an audio platform built on a minimal core (clock · graph interpreter · session log · context plumbing) with every capability as a plugin; the product is an assembled profile. First profile: the clip-arranger ("sound-arranger") — record long live jams, then cut, paste, rearrange, and shape them into a finished piece. Offline processing is a separate deferred profile ("sound sculptor"). Rust audio engine + Tauri v2 + Vue 3. Working research lives in [RESEARCH.md](RESEARCH.md); decision records live in [.agents/notes/](.agents/notes/README.md).

## Standing orders

- **Every non-trivial change adds or updates an Agent Note in the same commit.** Mechanical or local edits are exempt. Update the note that owns the decision; supersede, never rewrite, a decision.
- **Every note carries `## Alternatives considered`** — record what was rejected and why.
- **An implemented note states shipped reality in present tense.** Keep paths, names, and defaults current in the same change that alters them.
- **`.agents/notes/archived/` is frozen.** Never edit it; the pre-commit hook enforces this.
- **`RESEARCH.md` holds working research; notes own decisions.** When a decision locks, graduate it into a note, and link from research to the note instead of restating it.
- **The pre-commit hook runs `node scripts/verify-agent-notes.mjs`.** Node comes from the devenv shell; on a machine without node the hook warns and skips rather than blocking the commit.
- **External-model work is attributed** ([note](.agents/notes/implemented/process/2026-08-27-agent-attribution-convention.md)): a note/doc a model authored carries an `Authored with <model> · <harness>, <date>` footer; commits carry an `Assisted-by:` trailer. External reviews go verbatim under `research/architecture/` with a disposition header instead.
