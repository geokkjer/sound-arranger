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
  is a thin `git commit` wrapper that exports `GIT_AUTHOR_NAME`/`GIT_AUTHOR_EMAIL` for the model it
  is told to commit as and leaves `GIT_COMMITTER_*` untouched, so a commit records **author =
  model, committer = the human's configured identity**. `git log` and `git blame` now name the
  model; the human stays visible as the committer.
- **The model and harness are switch arguments, not constants.** `--model` / `--harness` (or
  `$DSH_AGENT_MODEL` / `$DSH_AGENT_HARNESS`) name whichever model and harness actually ran; the
  email is derived from the model — lowercased, each run of non-alphanumerics collapsed to one
  dash (`DeepSeek V4 Flash` → `deepseek-v4-flash`) — so a new model needs no code change, only the
  right label. The wrapper echoes the identity it is about to claim on stderr, so a wrong label is
  visible at commit time rather than discovered in the log afterwards.
- **The trailer is generated, not remembered.** The wrapper exports
  `DSH_AGENT_TRAILER="Assisted-by: <model> · <harness>"` and `.githooks/commit-msg` appends it
  verbatim when the message carries no `Assisted-by:` trailer yet, so a commit's identity and its
  disclosure cannot drift apart. The legacy hardcoded DeepSeek author still gets the legacy
  trailer. Human commits are untouched, and a model identity set by hand with no trailer is warned
  about on stderr rather than blocked.
- **The email is a subaddress of the human's own domain** (`<model-slug>@geokkjer.eu`), so an agent
  identity is clearly non-human, greppable, and needs no external account.
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
- **A per-model registry file** (key → name, email, trailer) — one more file to keep in sync for
  what two arguments already express, and it would still need the caller to name the right key.
  Rejected for now; revisit if the roster stabilizes and the spellings start to drift.
- **Auto-detecting the model from the harness environment** — no harness here exports the running
  model, and a guess that is wrong is precisely the failure the addendum below responds to.
  Rejected: the label must come from something that knows.

## Addendum (2026-09-27) — one switch, many models

The mechanism above pinned one model and one harness: the wrapper exported the DeepSeek identity
unconditionally, and the hook only attributed commits whose author name was exactly
`DeepSeek V4 Flash`. That was correct while one model committed here, and silently wrong the
moment a second one did. It broke in practice: a commit authored elsewhere — a different model, in
the **opencode** harness — was labelled by hand as `Claude (OpenCode)`, because the trailer was
typed rather than generated. Nothing in the tooling could have caught it, because nothing in the
tooling knew which model was running.

The switch makes the true label the cheap path: `--model`/`--harness` (or the environment), one
derived email, one generated trailer, and the claim echoed on stderr at commit time.

**What it cannot do, stated plainly:** no hook can verify that the model named is the model that
ran. The disclosure is only as honest as the label the caller passes, which is why the standing
order says never to label a commit with a model the switch was not told about — a wrong label is
worse than none, because it turns an unknown into a plausible fact. **Deliberately unchanged:** the
human remains the committer, identities still derive from the human's domain, the trailer stays
generated, and no history is rewritten.

## Consequences

- `git log`, `git blame` and `git shortlog -e` attribute agent work to the model; the human remains
  as committer, and GitHub shows the model as an **unlinked** author (no contribution-graph
  credit — intended).
- Agents commit through `scripts/agent-commit`; a plain `git commit` still produces a
  human-authored commit (the hook leaves it untouched), which is correct for manual commits.
- The `Assisted-by:` trailer is now generated rather than typed; the note/doc `Authored with`
  footers remain manual.
- The identity string and trailer name are a versioned snapshot: re-evaluate them when the model
  or harness changes (the co-work routing note's re-evaluation duty applies). Since the 2026-09-27
  addendum the snapshot is per-invocation rather than baked in, so this duty is discharged by
  passing the right `--model`/`--harness` rather than by editing the wrapper.
- The trailer's spelling now follows the model string as given, so the default reads
  `Assisted-by: DeepSeek V4 Flash · DeepSeek Harness` where commits before 2026-09-27 carry
  `DeepSeek-v4-flash · DeepSeek Harness`. A grep for attribution should match both spellings.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-12. Addendum authored with
DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-27.
