# Agent Note: The public front door — what the README owes a stranger

Status: implemented

## Problem

The repository went public on 2026-09-29, which changed what the README is *for*. It had been an
orientation note for the project's one human; it is now the page a stranger judges the project by,
and it had drifted in ways that matter much more in public than in private:

- **It contradicted itself about the most important capability.** One paragraph said recording works
  and is wired (`record <take_id>` … `record stop`), another said `record` stays unwired and
  `HostCommand::Record` is a stub. The first is right and has been since 2026-09-23; the second was
  the pre-alpha status paragraph, never updated.
- **It named the arranger as profile #1** and pointed at the umbrella-first note as "the direction",
  four days after [the profiles decision](../../proposed/architecture/2026-09-27-profiles-and-the-umbrella-name.md)
  made the **recorder** the focus and the platform `audio`.
- **It quoted numbers with no maintainer**: test binaries and test counts (235 when written, 319
  today), spike test counts, module line counts. None rotted through carelessness; counts in prose
  simply have nothing keeping them true.
- **It described the development gates as they were**: the pre-commit hook as notes-only (it now also
  runs `cargo fmt --all --check`, and the verifier resolves cross-references), and the command script
  as "the same one the Tauri shell bridge runs" (that shell was retired 2026-09-22).
- **It said nothing about** CI, the branch/pull-request discipline, the pre-alpha tag, or the
  attribution practice — which is exactly what a visitor uses to decide whether the project is worth
  reading further.

## Decision

Rewrite it as a public front door, keeping what was already good (the documentation map and the
development section) and fixing what would mislead a stranger:

- **What it is, first**: the platform, the core-plus-plugins shape, and the three profiles as a table
  with their real state — recorder (the focus, clock-out and alignment unbuilt), arranger (works end
  to end), sculptor (deferred) — linking the decision rather than paraphrasing it.
- **Status: pre-alpha**, with the tag named, and a pointer to
  [capabilities.md](../../../../docs/capabilities.md) for what works, what is cut deliberately, and the
  known-flaky list — the honest page, not a feature page.
- **A Try-it that was run**: the example script was executed while writing this and the result
  checked (96,000 frames, 2 channels, 16-bit, peak 0.172 — it makes sound), so the first command a
  visitor types is known to do what the page says. The hardware and soak commands stay marked as
  needing hardware.
- **No counts.** The suites and the invariants they pin are named; the arithmetic is not. That is the
  one class of claim on this page that cannot be kept true.
- **Development as it is now**: rustup-declared toolchain, the hooks (formatting + notes structure +
  cross-references, each skipping when its tool is missing), CI as the authority and what it runs,
  and the branch/PR discipline — merges are not squashed, docs may go straight to `main`, releases
  are tags.
- **How this is built**: decision notes with the rejected alternatives, model-assisted development
  disclosed per commit (model as author, human as committer, `Assisted-by` trailer), and flaky tests
  named rather than retried.

## Alternatives considered

- **Update the page in place.** Rejected: the contradictory paragraphs were the *framing*, not typos,
  and half of them would have survived a patch — a reader would still meet "record stays unwired"
  three lines after "recording works".
- **Split the front door into README + CONTRIBUTING.** Deferred: at this size one page serves both,
  and a second file is a second thing to fall out of date. The development section can graduate when
  there is an audience for it.
- **Quote fresh test counts instead of dropping them.** Rejected: they were stale by 84 tests and
  nobody was careless. Replacing 235 with 319 buys one accurate afternoon.
- **Add a GitHub Actions status badge.** Rejected for now: it is the first link to break when the CI
  moves off GitHub, and the page already says what CI runs, with a link to the workflow file. A badge
  is worth adding once CI has a stable home.
- **Keep calling the repo by its working title alone.** Rejected in favour of naming both: the
  repository is `sound-arranger`, the platform is `audio`, the rename is decided and not done, and
  saying so is cheaper than a reader wondering which name is real.

## Consequences

- A stranger's first screen answers: what it is, what state it is in, what works, what does not, how
  to run it, and how the project makes decisions.
- The worst contradiction is gone — the page no longer says recording works and that recording is
  unwired.
- **The page carries a freshness banner pointing at the code commit it was verified against**
  (`4a3706f`), so the next reader knows what "verified" means here: the claims were checked against
  the code at that commit, not against the previous README.
- Every link in the page resolves (checked), and the example is one that was run rather than written.
- The counts are gone, which is the only fix for that class of rot that does not need repeating.
- The front door and [capabilities.md](../../../../docs/capabilities.md) now tell the same story in the
  same order: what works, what is deliberately cut, what is known to be shaky.

*Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-29.*
