# Agent Note: The merge gate moves off Kimi — GLM-5.3 takes it

Status: implemented

## Problem

The roster of 2026-09-27 put **Kimi K3 on the merge gate** and GLM-5.3-Flash on the cheap cross-vendor
value pass and scoped coding handoff. Two attempts since then ran out the gate's five-hour limit
**without producing an answer at all** — not a wrong review, no review. The slices it was asked to
gate are ordinary ones (a node-width change, a CI workflow), so the mismatch is between our working
increment and that model's session budget, not between our work and its judgement.

Meanwhile the model that *has* been producing is GLM-5.3-Flash: it implemented the rig declaration and
the clock-out slice from written specs, and its reports have been accurate enough that reviewing them
was cheap. A gate that never answers is not a gate; a handoff model that works is the obvious
candidate to promote.

## Decision

**GLM-5.3 (full, on the `zai` route) gates merges. Kimi K3 leaves the loop. Everything else stands.**

- **Driver / planner**: DeepSeek-V4.1-Flash (unchanged).
- **Cheap independent value pass and scoped coding handoff**: GLM-5.3-Flash (unchanged).
- **Merge gate**: **GLM-5.3**, the full model rather than the Flash — the gate is a depth task, and
  the sibling that can catch the Flash's mistakes is the stronger sibling.
- **Kimi K3 is out of the roster**, not retired as an identity: it remains a legitimate name for the
  commits and reviews it already produced, and unlike the retired DeepSeek-V4-Flash there is no
  attribution problem to repair. It is simply not a route we use.
- **Route economics, since both routes are now live**: on `zai`, GLM-5.3-Flash is the cheap tier; on
  `opencode-go`, `glm-5.3-flash` is billed at **2× usage** while `glm-5.3` (full) is 1×. So the Flash
  belongs on `zai` and, when opencode-go is the route in play, the **full** model is the economical
  choice.

## The honest wrinkle this creates

The independence rule from [the roster note](2026-09-27-model-roster-re-evaluation.md) is that a
reviewer must not share the author's family — it is what places every review role. With GLM both
writing the handoffs *and* gating them, that property holds for diffs **DeepSeek-V4.1-Flash authors**
(the majority: the driver writes most changes directly) and is **lost for GLM-authored diffs**, where
the gate becomes a within-family review by a stronger sibling.

Recording it plainly: a within-family gate is not self-review — the chain for a handoff crosses three
parties (a DeepSeek spec, a GLM-Flash implementation, a GLM-5.3 review) — but it is not the
cross-vendor independence the rule asks for either, and calling it that would be the kind of
plausible-but-wrong label this project keeps refusing to write.

The candidate fix is a **third vendor**, and one exists: `opencode-go/hy4-preview`. It is a preview
model, so it is a candidate for a cheap independent *pass* first, and for the gate only once "preview"
stops being part of its name. That is a thing to try, not a thing to assume.

## Alternatives considered

- **Keep Kimi K3 and prompt it differently.** Rejected: two attempts produced no answer at all, and
  the failure is budget-shaped rather than prompt-shaped. A gate that only sometimes answers makes the
  merge discipline depend on luck.
- **GLM-5.3-Flash as the gate**, since it is already in the loop and cheap. Rejected: the gate is
  where depth pays, and using the same model *instance* that wrote the diff would be self-review
  rather than a weaker form of independence.
- **Make the gate a different vendor from the handoff by moving the handoffs off GLM.** Considered and
  not done: GLM-5.3-Flash is the model that has actually been working, and swapping a proven handoff
  to protect a property in the minority case is a bad trade. Revisit if handoffs become the majority
  of merges.
- **Route DeepSeek-V4-Pro in as the depth tier and gate instead.** Not currently possible: the
  `deepseek-official` provider advertises **only** `deepseek-flash` (V4.1-Flash) to subagents, so the
  named depth/escalation tier has no route today. The catalog is advisory, so an unlisted id *might*
  work — that is a thing to test, not to write into a table.
- **Silently drop the independence caveat** because the practical difference is small. Rejected: the
  review roles in this project are placed *by* that rule, so a change that weakens it is a change to
  the reasoning, not a footnote.

## Consequences

- Merges have a gate that answers within the working session, which is the property that was actually
  missing.
- **The independence rule now holds conditionally**, and the condition is written down: it holds for
  DeepSeek-authored diffs (most of them) and is suspended for GLM-authored ones, with the reason and
  the mitigation recorded rather than implied.
- Route guidance is now concrete: Flash on `zai`; on `opencode-go`, prefer the full GLM-5.3, because
  the Flash costs double there.
- `hy4-preview` is the named candidate for restoring cross-vendor review on GLM work, to be tried as a
  pass before it is trusted as a gate.
- The roster note of 2026-09-27 keeps its Decision as written; this note supersedes its gate row and
  is linked from the standing orders.

*Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-29.*
