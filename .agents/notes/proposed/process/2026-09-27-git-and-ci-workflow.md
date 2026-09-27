# Agent Note: The git and CI workflow — slices on branches, gates in CI, worktrees for agents

Status: proposed

## Problem

Today everything lands directly on `main`, there is **no CI at all**, and the only gate is a
pre-commit hook. That has been adequate, and it has three properties we should not keep relying on:

1. **The hook is local config.** `core.hooksPath` does not travel with a clone, so a fresh checkout
   — or the laptop — can commit without running any gate. The hook even warns and skips when node
   or cargo is missing. It is a courtesy to the next reader, not an authority.
2. **`main` is the only branch, and it is unprotected.** Nothing stops a force-push, and nothing
   verifies a commit before it is the thing the other machine pulls.
3. **Concurrent writers share one working tree.** This week two agents worked in the same checkout
   (a scoped coding handoff plus a documentation pass); a formatter sweep ran over a file the
   handoff had just finished. Nothing broke, and that was luck plus small diffs, not design.

None of this is about process for its own sake. It is about the two failure modes this repo has
already met: a second machine (the session opened with "I did some work on my laptop, can you
pull"), and more than one writer at a time. Both get worse, not better, as the alpha approaches.

## Proposal

Five stages, each useful on its own, each reversible. Stages 1 and 2 are the ones that pay
immediately; nothing here waits for the alpha.

**1. CI becomes the authority** (`.github/workflows/ci.yml`). The hook stays, as a *subset* of CI —
same commands, faster feedback:

```yaml
name: ci
on:
  push: { branches: ['**'] }
  pull_request:
concurrency: { group: ${{ github.ref }}, cancel-in-progress: true }
jobs:
  rust:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: 'rustfmt, clippy' }
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
  notes:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: '22' }
      - run: node scripts/verify-agent-notes.mjs
```

Three deliberate choices: **Linux only** (the audio stack is ALSA/cpal and the studio is Linux;
the hardware tests are `--ignored` and cannot run in CI anyway), **the spikes get their own jobs**
(`spikes/tui-shell` and `spikes/iced-shell` are separate workspaces, so a root-workspace test run
does not cover them), and **`crates/shell` is never built** — it is retired and frozen
(`crates/shell/RETIRED.md`), and CI should not resurrect it.

**2. `main` becomes protected and always green.** Require the CI jobs, refuse force-push, and
refuse deletion. This is a GitHub setting, so it is the one part of this note a human has to do.
The rule it buys: whatever the laptop pulls, builds and tests.

**3. Work happens on short-lived branches, one per slice.** `slice/<topic>` (the repo's unit of
work is already a slice with a note), `fix/<topic>` for a repair, `docs/<topic>` for a docs pass.

- **Branch now; tag at the alpha.** Branching is not a milestone reward — the failure it prevents
  is already happening. What *should* wait for the alpha is releases: a tag (`v0.1.0-alpha.1`) and
  a release note assembled from the notes, not a `release/*` branch.
- **PRs are the review surface.** The PR body carries the note; the review gate (Kimi K3 for
  merge gates, GLM-5.3-Flash for a cheap independent pass) reviews the **diff** and its verdict
  goes in the PR conversation — and, when it is a formal gate, verbatim under
  `research/architecture/` with a disposition header, exactly as reviews already do.
- **Merge without squashing** (rebase-merge or a merge commit). Squashing would erase the
  per-commit `Assisted-by` attribution this repo deliberately records, and flatten a slice's
  commit story into one blob.
- **Docs and typo fixes may still go straight to `main`** when CI is green. The risk lives in code
  and contracts, not in change size; a rule that requires a PR for a comma is a rule people route
  around.

**4. Worktrees for concurrent writers.** One tree per agent session, the main checkout left clean
on `main`:

```bash
git worktree add ../sa-rig -b slice/rig-declaration    # an agent works here
git worktree list                                      # who is where
git worktree remove ../sa-rig                          # after the merge
```

Each worktree gets its own `target/` by default (correct, and disk is cheap). A shared
`CARGO_TARGET_DIR` saves rebuilds but serialises concurrent `cargo` invocations on the build lock —
worth it only when the sessions are sequential.

**5. The gates keep their promises.** The hook stays advisory-when-missing; CI never is. A tool
missing locally produces a warning and a commit; the same state in CI is a red build. Documented
in AGENTS.md so the difference is not folklore.

## Alternatives considered

- **Stay on trunk, direct to `main`, no CI (the status quo).** Rejected as the *only* mode, not as
  a mode: it is fine for a single writer and nothing to protect, and both assumptions have already
  been false this month. It also makes the hook the last line of defence, which it cannot be — it
  does not travel with a clone.
- **GitFlow (`develop`, `release/*`, `hotfix/*`).** Rejected: a release train for a project with no
  releases, a long-lived integration branch is a conflict factory, and the slice is already the
  natural unit of work.
- **Squash-merge every PR.** Rejected: it destroys per-commit model attribution and the multi-commit
  slice narrative. Attribution is not decoration here; it is the record of who wrote what.
- **Require a PR for every change, including docs and typos.** Rejected: friction with no matching
  risk, and its predictable outcome is that people batch unrelated changes into one PR to amortise
  it.
- **Trunk-based development with feature flags.** Rejected for now: there is nothing to flag — the
  work is additive behind plugin seams, and no user-facing surface is live.
- **CI on macOS and Windows too.** Deferred: no code path is platform-specific today (hardware
  tests are `--ignored`), and a matrix that only proves "it also compiles" is minutes for a feeling.
- **A self-hosted runner on the studio machine.** Deferred: nothing needs device access yet. If a
  hardware-in-the-loop test ever does, the right shape is a separate, explicitly-triggered job, not
  the default gate.
- **Move the hook's checks into a `just`/`make` task and drop the hook.** Rejected: the hook is the
  only check that runs without being remembered, and it costs nothing to keep as a fast subset.

## Acceptance criteria

1. CI runs formatting, clippy (`-D warnings`), the workspace tests and the notes verifier on every
   push to a branch and every PR; the two shell spikes are covered by their own jobs; `crates/shell`
   is not built.
2. `main` requires those jobs, refuses force-push and deletion.
3. A slice lands as a branch plus a PR whose body carries its note; the merge preserves per-commit
   authorship and `Assisted-by` trailers (verified once, on a real branch, not assumed).
4. `git worktree` is the documented convention for concurrent agent sessions, and the main checkout
   is clean on `main` between sessions.
5. The alpha is a tag plus a release note, never a release branch.
6. CI runtime stays under roughly five minutes warm, so it is a gate and not a queue.

## Risks

- **Green CI that still misses the point.** CI proves the code compiles, lints clean and passes its
  tests; it cannot check that a *decision* is sound. The review gate stays load-bearing — CI is
  necessary, not sufficient.
- **Branch sprawl.** Small risk, cheap fix: delete on merge, and `git worktree remove` when a
  session ends. A stale branch is only dangerous if it is merged late.
- **Attribution through GitHub's merge.** A rebase-merge should preserve author and trailers, but
  the only honest way to know is to try it once on a disposable branch and inspect
  `git log --format='%an <%ae> / %cn <%ce>'`. If it mangles anything, use a merge commit and record
  why.
- **Hook/CI divergence.** Two places listing the same commands will drift. Mitigation: the hook runs
  the *same* commands, and CI is authoritative when they disagree.
- **The alpha temptation.** "Wait for the milestone" is the comfortable version of this decision;
  the milestone is where *releases* start, and the drift this prevents does not wait for it.

*Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-27.*
