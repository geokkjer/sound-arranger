# Agent Note: No outside contributions before beta

Status: proposed

## Problem

The repository is public and its workflow documentation already describes how a change lands — a
short-lived branch, a pull request, an unsquashed merge ([workflow
note](../../implemented/process/2026-09-27-git-and-ci-workflow.md)). Nothing anywhere says whether an
*outside* pull request will be accepted. A reader who finds the repo, reads that workflow and invests
a weekend in a patch is being set up to have it closed.

There is a second, quieter stake. Every commit to date was authored by the human or his own agent
identities, so **a single copyright holder owns the whole work** — verified 2026-09-30 across all
commit authors and committers. That is what makes the licence a live decision: the project can adopt
any licence in one step. The first accepted outside contribution ends that permanently, because
relicensing then requires every contributor's consent. The current position is `GPL-3.0-or-later`
with the `-or-later` clause deliberately kept so that an upgrade to AGPL-3.0 remains possible
unilaterally ([RESEARCH.md](../../../../RESEARCH.md) §12).

## Proposal

**Outside contributions are not accepted until there is a beta to contribute to. Pull requests will
be closed unmerged, and the positions that remain open — issues, forks — are stated openly in the
[README](../../../../README.md) `## Contributing`.**

1. **The gate is a focus decision first.** There is no stable API to build against while the
   architecture, the licence and the project's name are still moving (the rename is unresolved —
   [RESEARCH.md](../../../../RESEARCH.md) §14). A patch against a surface about to change costs the
   contributor more than it costs the project.

2. **It is also what keeps the licence a live decision.** The gate protects the sole-copyright-holder
   position and therefore the ability to relicense in one step. This is a reason, not the reason —
   reverse-engineering it as the sole motive would overstate it. But it is the concrete cost of
   lifting it, so it belongs in the record.

3. **What is open, plainly.** Issues are welcome and are the intended way in — no CLA, no commitment.
   Forking is explicitly fine; the licence already grants it. The only thing withheld is the push
   back upstream.

4. **The gate lifts at beta, and the contribution policy is decided *before* it lifts, not after the
   first PR arrives.** That policy will be a **DCO** rather than a CLA. A DCO is a per-commit
   `Signed-off-by` assertion; a CLA is a signed agreement plus, in the usual form, a copyright
   licence or assignment granting the maintainer the right to relicense. Because this project
   actually values the unilateral-relicense option, it is a candidate for the *heavier* CLA — but
   that is a decision to take deliberately at beta with the tradeoff in front of us, against the
   deep discouragement a CLA imposes on casual contribution. Recording the leaning now so the
   tradeoff is not discovered late.

## Alternatives considered

- **Accept outside contributions now, under a DCO** — rejected: it spends the relicense option
  immediately and invites patches against an unstable surface. The upside (help) is not scarce at
  this size, and the project already runs a model-assisted development loop.
- **Accept contributions now under a CLA, preserving the relicense option** — rejected: it puts the
  heaviest possible ceremony in front of a pre-alpha project with no stable API, which is the worst
  ratio of friction to value. It also would have to be written and administered now, for
  contributions that are not wanted yet.
- **Stay silent and adjudicate PRs case by case** — rejected: that is the setup for wasted
  contributor effort, and it makes an intentional policy look like rudeness. Being upfront costs one
  README section.
- **Relicence now to remove the question** — rejected: the programme is explicitly pre-alpha and aims
  to stay `-or-later` precisely so the decision can be deferred on evidence. Spending the option
  early to simplify a README is a bad trade.
- **Add a `CONTRIBUTING.md`** — rejected for now: the policy is four short points, and the README is
  where a visitor already is. A separate file is justified when there are build instructions,
  test requirements and a style guide to state — that is beta-era work.

## Acceptance criteria

- [README.md](../../../../README.md) `## Contributing` states that PRs are closed unmerged before
  beta, that issues and forks are open, and why the gate exists.
- The `Assisted-by` attribution convention is unaffected: it describes work already accepted, not a
  route for new contributors.
- The [licence rationale](../../../../RESEARCH.md) §12 is unchanged and still governs; if the gate is
  lifted or the licence changes, that is a new note superseding this one rather than an edit.
- At beta, a note chooses DCO or CLA **before** the first outside PR is accepted.

## Risks

- **The policy reads as unwelcoming.** Mitigated by naming the open routes (issues, forks) and the
  concrete trigger (beta) rather than a blanket "no contributions". An unexplained closed door is
  worse than an explained one.
- **Beta may be far off**, so the policy may stand for a long time and deter exactly the person whose
  bug report would have been useful. The issues route is the mitigation, and it needs to be actually
  read — a stale issue tracker would make the whole posture hollow.
- **The sole-copyright position is only as good as the commit record.** The agent-commit convention
  keeps authorship non-human but the committer human, and the [attribution
  convention](../../implemented/process/2026-08-27-agent-attribution-convention.md) already
  anticipates a wrong label being worse than none. If a contributor's patch ever lands by accident,
  this note's premise (and the relicense option) changes silently.
- **A DCO is not a copyright licence.** It cannot grant the right to relicense. If the project still
  wants that option at beta, it will need a CLA and the friction that implies — which is why item 4
  names the tension rather than resolving it early.
