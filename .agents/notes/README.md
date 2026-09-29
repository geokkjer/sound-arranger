# Agent Notes — sound-arranger

Agent Notes are short decision records: the *why* behind a choice, what was considered and rejected, and what it costs. They are the project's memory for decisions, written at decision time so future you (and future agents) don't re-litigate or forget. This file defines where they live, when to write one, and the format.

## Layout and naming

Every note lives at `.agents/notes/{lifecycle}/{class}/yyyy-mm-dd-topic.md`.

- **Lifecycle** is the note's status and the folder it sits in:
  - `proposed/` — decided or under consideration, but not yet built. Speaks in future tense.
  - `implemented/` — the decision shipped in code. Rewritten to present tense and kept current with what actually exists.
  - `rejected/` — considered and declined. The verdict lives on the `Status:` line. Delete it once the rationale no longer prevents a tempting mistake.
- **Class** is the kind of decision: `feature` (a new capability), `bug-fix` (a defect corrected), `simplification` (removed code or surface area), `architecture` (structure of the shipped source), `process` (tooling and workflow around the code).
- The date in the filename is when the topic was first proposed.

The tree itself is the inventory — there is no index file. Search or browse instead.

## When to write one

Write or update a note in the same commit as any non-trivial change: new behavior, a structural change, a contract or format change, a tooling/process change, or any decision you might reasonably revisit. Purely mechanical or local edits are exempt.

Update the note that already owns the decision; do not duplicate it. Never edit a note into a *different* decision — write a new note and cross-link (relative markdown links, so they survive moves). **When two notes disagree, the later decision governs**: the theory of the program is being refined, not excavated, so precedence is temporal and superseding means a cross-link back from the new note to the one it overrides. `RESEARCH.md` holds working research; a note holds the decision. When research locks a choice, graduate it into a note; research can link to the note instead of restating it.

## File format

The first three lines are exactly:

```markdown
# Agent Note: <title>

Status: <proposed | implemented | rejected — why, in one line>
```

The body opens with `## Problem` — the motivation, written to stand without the solution. The rest depends on the lifecycle:

- `proposed/`: `## Proposal` → (bespoke sections) → `## Alternatives considered` → `## Acceptance criteria` → `## Risks`. The proposal may speak in future tense; acceptance criteria say what observable state means done.
- `implemented/`: `## Decision` (present tense, what actually shipped) → (bespoke sections) → `## Alternatives considered` → `## Consequences`. No `## Proposal`/`## Plan`/`## Acceptance criteria` — those are proposal-era spec-speak.
- `rejected/`: the proposal frozen as it was; the verdict is on the `Status:` line.

`## Alternatives considered` is mandatory in every note — a decision recorded without what it beat invites re-litigation. One bold-led paragraph per alternative is enough.

Moving a note between lifecycles means updating the `Status:` line and re-satisfying that lifecycle's sections in the same change. `proposed/` → `implemented/` rewrites `## Proposal` into a present-tense `## Decision` and folds acceptance criteria and risks into `## Consequences`.

## Verification

`node scripts/verify-agent-notes.mjs` checks the whole tree: closed lifecycle/class sets, filename grammar, the header block, required sections, the mandatory alternatives section, and that every cross-reference resolves. It runs automatically from the `pre-commit` hook (`.githooks/`, wired via `core.hooksPath`); you can also run it by hand.

`archived/` is a frozen history by convention: once a note moves there it is never edited. The pre-commit hook refuses any change under `.agents/notes/archived/`.
