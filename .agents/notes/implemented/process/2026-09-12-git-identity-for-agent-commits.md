# Agent Note: Model git identity for agent commits — author is the model, committer is the human

Status: implemented

## Problem

Every commit in this repo is authored by the human (`Geir Okkenhaug Jerstad
<geir@geokkjer.eu>`), yet the DeepSeek-V4-Flash agent writes essentially all of the code, the
docs, and the notes. The only model-attribution signal is a hand-typed `Assisted-by:` trailer
([attribution convention](2026-08-27-agent-attribution-convention.md)); it is easy to forget — it
was missed on the hub-and-spokes and phase-sequence notes' first landing — and `git log`,
`git blame` and `git shortlog` all report the work as the human's.

A configuration-only fix does not exist. Git's conditional-config "profiles"
(`includeIf "gitdir:…"`, `onbranch:`, `hasconfig:remote.*.url:`) key on the *path, branch or
remote*, never on *who is running the command* — and the human and the agent commit in the same
checkout. The actor must therefore set the identity.

## Decision

- **Agent commits are authored by the model and committed by the human.** `scripts/agent-commit`
  is a thin `git commit` wrapper that exports `GIT_AUTHOR_NAME=DeepSeek V4 Flash` and
  `GIT_AUTHOR_EMAIL=deepseek-v4-flash@geokkjer.eu` and leaves `GIT_COMMITTER_*` untouched, so a
  commit records **author = model, committer = the human's configured identity**. `git log` and
  `git blame` now name the model; the human stays visible as the committer.
- **The trailer is generated, not remembered.** `.githooks/commit-msg` appends
  `Assisted-by: DeepSeek-v4-flash · DeepSeek Harness` whenever the model identity is the author
  and no `Assisted-by:` trailer is present. Human commits are untouched.
- **The identity string is** `DeepSeek V4 Flash <deepseek-v4-flash@geokkjer.eu>` — a subaddress of
  the human's own domain, so it is clearly non-human, greppable, and needs no external account.
- **Documents keep the `Authored with …` footer** ([convention](2026-08-27-agent-attribution-convention.md));
  the git identity replaces only the *commit* half of that convention, which it extends rather
  than supersedes.
- **No history rewrite.** Existing commits keep their human author; the identity applies from this
  commit forward, so hashes stay stable.

## Alternatives considered

- **`includeIf` path/branch profiles (true "git profiles")** — rejected: they switch config by
  repo path, branch or remote, not by actor, and both the human and the agent commit here. They
  would work only with a separate agent worktree — a workflow change this does not need.
- **`git config --local user.name` in the repo** — would apply to the human too, misattributing
  every manual commit. Rejected.
- **Both author *and* committer = model** — rejected: it removes the human from every commit
  entirely, losing "who applied it" and the oversight the committer role records.
- **`Co-authored-by:` trailer only (author stays human)** — rejected as the primary mechanism: it
  keeps the model out of `git blame`/`git log`, which is where attribution is actually read, and
  GitHub renders it only if the email links to an account.
- **A dedicated GitHub bot account's noreply email** — the only way to get commits *linked* on
  GitHub. Rejected for now: it needs an account created and maintained, while the subaddress is
  enough for the repo's transparency goal (attribution here is process transparency, not legal
  authorship — see the [convention](2026-08-27-agent-attribution-convention.md)).
- **Fail the commit when the trailer is missing instead of appending it** — rejected: auto-append
  removes the failure mode; a hook that only scolds still lets commits through when it is absent.
- **Rewrite existing history (`filter-repo`) with the model identity** — rejected: it changes
  every hash and breaks clones and open PRs for a retrospective labelling gain.

## Consequences

- `git log`, `git blame` and `git shortlog -e` attribute agent work to the model; the human remains
  as committer, and GitHub shows the model as an **unlinked** author (no contribution-graph
  credit — intended).
- Agents commit through `scripts/agent-commit`; a plain `git commit` still produces a
  human-authored commit (the hook leaves it untouched), which is correct for manual commits.
- The `Assisted-by:` trailer is now generated rather than typed; the note/doc `Authored with`
  footers remain manual.
- The identity string and trailer name are a versioned snapshot: re-evaluate them when the model
  or harness changes (the co-work routing note's re-evaluation duty applies).

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-12.
