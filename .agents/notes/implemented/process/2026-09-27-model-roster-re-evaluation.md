# Agent Note: Co-work roster re-evaluation — V4.1-Flash drives, V4 Flash is retired

Status: implemented

## Problem

The [co-work routing note](2026-08-27-model-co-work-routing.md) is a versioned snapshot of who
plays which role, and it instructs a re-evaluation of the whole table on each new model or material
price/capability shift, recorded as a new note. Two shifts have landed since 2026-08-27:
**DeepSeek-V4-Flash retired**, and a successor — **DeepSeek-V4.1-Flash** — is already the
deployment's default model. The table therefore names a retired model in the driver seat, and the
next agent to read it would try to route work to something that cannot be invoked.

The retirement reaches past routing. The identity tooling's only default was `DeepSeek V4 Flash`,
so a new commit could still claim a model that no longer exists; that half is handled in the
[git-identity note](2026-09-12-git-identity-for-agent-commits.md), which now requires the model to
be named and treats the retired name as anonymous.

## Decision

- **Row 0 (driver / planner) moves to DeepSeek-V4.1-Flash.** It is what this deployment already
  runs — `~/.dsh/settings.yaml` defaults `agent-default-model` to `deepseek-flash`, displayed as
  DeepSeek-V4.1-Flash — and it is the successor to the retired model. The role is unchanged:
  continuous-session driving, planning, editing, and delegation, at the cheap high-volume tier.
- **DeepSeek-V4-Flash is retired, and retired is a routing fact.** No role may name it, and any
  artifact still carrying its name is **anonymous** rather than attributable — there is no model
  left that can legitimately claim it.
- **The independence rule is unchanged, and it still decides the reviewer roles.** To judge a
  DeepSeek model's work, use a non-DeepSeek model; Kimi K3 remains the merge gate and GLM-5.3-Flash
  the cheap cross-vendor value pass. Nothing about a retirement changes that reasoning — if
  anything it matters more, since driver and escalation tier are now the same family.
- **Rows 1–3 are re-affirmed on that rule alone, not on fresh measurement.**

## What this re-evaluation did not establish

The table mixes two kinds of claim: **role assignment** (which the retirement forces a revisit of)
and **price/capability placement** (which it does not, and for which there is no fresh measurement
here). Rows 1–3 stand because the independence argument places the *review* roles, not because
anyone re-priced them; V4-Pro stays the depth/escalation tier and stays out of the reviewer roles
for the same-family reason it always did. Calling this a full re-evaluation would overclaim. The
honest statement: the roster changed, the reasoning that assigns the review roles did not, and the
price placement is due for a real pass when someone has the numbers.

## Alternatives considered

- **Edit the 2026-08-27 note's table in place** — rejected: that note's Decision is the record of
  what was decided then, and "supersede, never rewrite" is the standing order. It gets a pointer.
- **Keep V4 Flash in the table as a historical row** — rejected: a routing table is something to
  act on, and a row nobody can invoke is a trap for the next agent that reads it.
- **Move the driver to DeepSeek-V4-Pro, the strongest DeepSeek tier** — rejected: the driver is the
  high-volume continuous role, and V4-Pro is deliberately neither driver nor reviewer here — it is
  the escalation tier for genuine difficulty.
- **Expand the table from the deployment's configured roster** — `settings.yaml` also lists
  GLM-5.3 (full), Kimi K3, and an `hy4-preview` route. Rejected for now: a capability list is not
  evidence about a role, and adding a model without a measured reason is how a table goes stale in
  the other direction.

## Consequences

- The driver tier and the git-identity default had to move together: the identity's only default
  *was* the retired model, which is why the same day's change makes `--model` required.
- Any footer, trailer, or note still naming the retired model reads as anonymous; grep both
  spellings (`DeepSeek V4 Flash`, `DeepSeek-v4-flash`).
- The cross-vendor requirement now carries more weight, not less: driver and escalation tier are
  both DeepSeek, so neither may review the other, and the value pass and gate must stay outside
  that family.
- Agreeing with the deployment default is not the same as measuring it. This note records a roster
  change; the price/capability pass remains open, and this note should be superseded rather than
  stretched when it happens.
