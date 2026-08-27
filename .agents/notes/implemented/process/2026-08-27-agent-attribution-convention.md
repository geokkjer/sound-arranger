# Agent Note: Attribution for external-model contributions

Status: implemented

## Problem

The project's history is its decision record, and several models now collaborate on it (kimi-cli reviews, GLM-5.3 Flash review passes and implementation work, a planned DeepSeek lead-review role). None of that authorship is visible in the artifacts themselves: git records one human identity regardless of which model drafted a change, and nothing in a note says who wrote it. Attribution added only in commit messages or chat would rot; the story of *which model did what* would not survive into the documents future agents actually read.

## Decision

Contributions authored or substantially drafted by an external model are marked in two places, as part of the change itself:

1. **Documents**: notes and docs the model authored carry an italic footer line — `Authored with <model> · <harness>, <date>` (e.g. `Authored with GLM-5.3 Flash · ZCode, 2026-08-27`). Documents merely edited or extended do not need one; substantial redrafts count as authored.
2. **Commits**: a `Assisted-by:` trailer naming the model and harness (`Assisted-by: GLM-5.3 Flash (ZCode)`).

External *reviews* keep their existing convention instead: verbatim archive under `research/architecture/` with a source-and-disposition header (the kimi/GLM precedent), cited from the note they fed. The footer marks drafting; the archived file marks reviewing.

## Alternatives considered

- **Commit-trailer only** — invisible where decisions are actually read (the notes tree), lost when histories are squashed or rebased. Rejected as the sole marker.
- **A CONTRIBUTORS/COLLABORATORS registry file** — centralized and easy to forget; attribution decays from the artifact it describes. Rejected.
- **No attribution** — simplest, and technically implied by copyright law (no authorship for tool output), but the user requirement is process transparency, not legal authorship: knowing which model shaped which decision is context for judging it. Rejected.

## Consequences

- Any note can be traced to its drafting model at read time; the convention started with the control→render handoff note (2026-08-27).
- Models with existing archives (kimi, GLM-5.3) predate the footer convention — their contributions stay attributed via the archive headers, unmodified.

*Authored with GLM-5.3 Flash · ZCode, 2026-08-27.*
