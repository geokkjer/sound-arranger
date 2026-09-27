# Agent Note: Model co-work routing — driver, value pass, escalation, reviewer gate

Status: implemented

> **Re-evaluated 2026-09-27:** DeepSeek-V4-Flash retired and row 0 (driver / planner) moved to
> DeepSeek-V4.1-Flash; the remaining rows stand on the independence rule, and the price placement
> is due for a real pass. See [the roster re-evaluation](2026-09-27-model-roster-re-evaluation.md).
> The table below is the 2026-08-27 snapshot, left as decided then.

## Problem

The project is built by one human plus several models, and the loop already runs in practice: DeepSeek-V4-Flash drives the session and plans; Kimi has reviewed every substantive slice since 2026-08-15; GLM-5.3 Flash did a one-off review pass and authored several docs on 2026-08-27. But the routing was implicit — carried in chat and commit trailers, not written down, and spread across a half-dozen review archives. With a fourth model entering (DeepSeek-V4-Pro, which the owner set up but could not place), the arrangement has outgrown what chat can hold, and two questions can no longer be answered from the record: *which model does what*, and *why that model*. The owner asked for the schema documented with a timestamp and a methodology, because the field is a moving target and routing will change.

## Decision

Run the co-work loop as a four-tier cascade along two axes, and write the routing down as the standing arrangement (the two newest placements running as trials until they prove out):

- **Axis 1 — difficulty / cost:** cheap-and-fast to expensive-and-deep.
- **Axis 2 — independence:** same-vendor (correlated blind spots) to cross-vendor (diverse blind spots).

| Tier | Role | Model | Vendor family | Independence | Used when | Cost posture |
|------|------|-------|---------------|--------------|------------|--------------|
| 0 | Driver / planner | DeepSeek-V4-Flash | DeepSeek | same-family | continuous session driving, planning, editing, delegating | cheap, high-volume |
| 1 | Value pass | GLM-5.3-Flash | Zhipu / Z.ai | cross-vendor | small second opinions, quick "is this sound?", scoped coding handoff | very cheap, fast |
| 2 | Escalation / depth | DeepSeek-V4-Pro | DeepSeek | same-family (depth only) | a knot that needs more reasoning than the driver can give cheaply | mid — cheaper than Kimi |
| 3 | Reviewer gate | Kimi K3 | Moonshot | cross-vendor | substantive and architectural slices, the merge gate | expensive, deep |

## Independence rule

A reviewer or a "second opinion" must not share a vendor with the model whose work it judges. Independence — not raw strength — is what a second opinion buys. Model families share training data, alignment, and priors, so a same-vendor reviewer tends to *confirm its sibling*; an outside reviewer catches the family's shared failure mode, which is precisely the class of bug that matters, because the driver did not see it *precisely because it is baked into how the family thinks*.

This one rule is what makes Kimi the gate and GLM the value pass, and what keeps DeepSeek-V4-Pro out of both roles:

- To review a DeepSeek model's work, use a non-DeepSeek model (Kimi or GLM).
- To review GLM's work, use DeepSeek or Kimi.
- Never have DeepSeek review a DeepSeek model when independence is the goal.

DeepSeek-V4-Pro is therefore disqualified from the reviewer-gate and value-pass *roles* for DeepSeek work — not because it is weak, but because its opinion is not independent.

## DeepSeek-V4-Pro placement

Its honest slot is **depth, not diversity.** It is a strong reasoner of the same family as the driver, so it earns its keep when the blocker is *difficulty* (a genuinely hard correctness or design knot) rather than *blind-spot detection*.

- **Escalate to it when** the driver or the value pass hits a knot it cannot cheaply close: a sample-accuracy proof, a realtime-safety invariant, an intractable seam or design tension, "is this property actually true under these edge cases?" Use it for deeper reasoning *before* spending Kimi's expensive tokens.
- **Not the reviewer gate** because of independence (above).
- **Not the value pass** because it is neither cheap enough nor independent — the value pass exists to be cheap *and* cross-vendor, and DeepSeek-V4-Pro is neither.

**Anti-pattern to avoid:** do not route a "second opinion" on the driver's work to DeepSeek-V4-Pro just because it is strong. That buys *depth on top of an already-correlated view* — it will tend to agree with the driver and miss the shared blind spot that GLM or Kimi would catch. If the question is *"did the driver miss something?*", that is a diversity question → GLM or Kimi. If the question is *"how do I actually solve this hard thing?*", that is a depth question → DeepSeek-V4-Pro.

## Routing methodology

- **Value-pass trigger (→ GLM-5.3-Flash):** local, well-scoped, bounded by the change — a single function, a unit-test gap, a narrow edit, a quick "is this sound?", a just-fixed-local-defect check. If GLM flags a real defect, or the change touches the core / realtime path, it survives upward.
- **Reviewer-gate trigger (→ Kimi K3):** substantive and cross-cutting — a plugin's contract, a log-schema or value-schema change, a PDC / seam / sample-accuracy decision, a slice of real feature work, "will this hold up under the invariants?" This is the expensive authority pass before merge.
- **Escalation trigger (→ DeepSeek-V4-Pro):** a knot of genuine difficulty that the cheap models demonstrably cannot close (see placement above).
- **Cost budget:** use the cheapest model that can do the job; escalate only when the cheaper one demonstrably cannot. Never send the trivial to Kimi. This conserves the expensive tokens for the gate.
- **Handoff protocol:** the driver scopes a bounded, self-contained brief with the note / conventions the work must satisfy before handing off. Handing to GLM for code means a closed brief; the human, the driver, and git review the result, and if it touches realtime / core correctness it still passes through the reviewer gate.
- **Escalation path:** driver → (value pass if cheap) → (escalation if hard) → (reviewer gate if substantive). The value pass and escalation are optional bands; not every problem climbs all four.

## How the loop actually runs (invocation)

The routes, as of 2026-08-27 (subject to the same re-evaluation as the table):

- **GLM-5.3-Flash (value pass) — the `pi` harness**, via `pi --provider zai --model glm-5.3-flash --print --no-session "<brief>"`. `pi`'s `settings.json` defaults `defaultModel` to `glm-5.3` (the **full** model) with `defaultThinkingLevel: xhigh` — so pass `--model glm-5.3-flash` explicitly, or the task goes to full GLM, not Flash.
- **Kimi (reviewer)** — `kimi-cli` non-interactive, with the fallback `dsh --profile headless` `opencode-go`/`kimi-k2.6` route when kimi-cli is rate-limited (per the review archives). Passes are archived verbatim under `research/architecture/`.
- **DeepSeek-V4-Flash (driver)** — the DSH session itself; no separate invocation.

**Sandbox caveat (the DSH workspace-write sandbox):** `~/.pi` is read-only inside the sandbox, so `pi` cannot create its `auth.json.lock` / `settings.json.lock` and aborts with `EROFS`, even though outbound API calls are **not** blocked. Workaround — give `pi` a writable home inside the repo:

```sh
PH="$PWD/.pi-home"
mkdir -p "$PH/.pi/agent" && cp ~/.pi/agent/{auth.json,models-store.json,settings.json,APPEND_SYSTEM.md} "$PH/.pi/agent/"
mkdir -p "$PH/.pi/agent/sessions" "$PH/.pi/agent/skills" "$PH/.pi/agent/missions" "$PH/.pi/agent/npm"
HOME="$PH" pi --provider zai --model glm-5.3-flash --print --no-session "<brief>"
```

`.pi-home/` holds a copy of the credential and is **untracked scratch** — it must not be committed: `.gitignore` covers it (`.pi-home/`), and it should be deleted after the run. The model id is passed on the command line, bypassing `settings.json`'s full-GLM `defaultModel`. The brief is the driver's home; the model id is the route; the `.pi-home` trick is what makes it runnable from inside the sandbox.

## Attribution & records

Each model's work is attributed per the [attribution-convention note](2026-08-27-agent-attribution-convention.md). In brief: external-model-authored notes and docs carry an italic `Authored with <model> · <harness>, <date>` footer; commits carry an `Assisted-by: <model> (<harness>)` trailer; external *reviews* go verbatim under `research/architecture/` with a source-and-disposition header. GLM handoffs therefore carry `Authored with GLM-5.3 Flash · <harness>, <date>`; Kimi reviews stay under `research/architecture/`. DeepSeek-V4-Flash drives the session and is the default author, so its notes carry no external-model footer (the convention's scope is external models).

## Evaluation & re-evaluation

This is a snapshot, not a law. The two trial placements (GLM-5.3-Flash carrying the value pass, DeepSeek-V4-Pro carrying escalation) are judged on a trigger, not a schedule:

- **GLM-5.3-Flash earns its slot** if its value-pass second opinions consistently catch real defects cheaply and its handoff code passes the reviewer gate without rework. If it misses what the gate then catches, demote it to occasional use. Its multimodal audio input adds a distinct value axis — verifying that a bounce/clip actually *sounds* right (silence, clicks, pops, pitch/speed drift) — which code review cannot reach; that is a second, independent way it earns the tier.
- **DeepSeek-V4-Pro earns its slot** if escalation genuinely unblocks problems the cheap models could not. If it mostly just confirms the driver (correlated agreement), drop it — that is the family-confirmation trap.
- **Re-evaluate the whole table** whenever the owner adds a model, or a model's price or capability shifts materially. Record the new snapshot as a fresh note — supersede, never rewrite — so the history of routing decisions is itself a decision record.
- **Freshness:** this note is timestamped by its filename (2026-08-27), and its model-characterization paragraphs (below) are explicitly "as of that date." Treat them as dated snapshots, not current facts.

### Model evidence (snapshot, 2026-08-27)

- **DeepSeek-V4-Flash** — the efficiency / fast tier of the DeepSeek V4 lineup; cheap and high-volume, the natural driver. (DeepSeek API Docs; SiliconFlow launch note.)
- **DeepSeek-V4-Pro** — the V4 **flagship**, marketed as moving toward the leading tier and compared head-to-head with Kimi K3; stronger and more expensive than Flash. (HK01 launch report; Artificial Analysis comparison.)
- **GLM-5.3-Flash** — Zhipu / Z.ai's value model (the "牛来" / NiuLai line): very cheap, ~1/40 of Opus 4.8, strong per-cost; the repo has already run it read-only via `pi --provider zai --model glm-5.3`. It is also Z.ai's first **natively multimodal** open-weights model — image, *audio*, and video input — which the tier-1 value pass can turn into **audio verification**: listen to a bounce/clip to catch silence, clicks, pops, or pitch/speed drift that code review cannot see. (36Kr; QQ tech; [apidog](https://apidog.com/blog/glm-5-3-flash-what-is/); [vLLM recipe](https://recipes.vllm.ai/zai-org/GLM-5.3-Flash).) The audio-verification extension is a candidate finding under the GLM evaluation trigger below.
- **Kimi K3** — Moonshot's frontier model, matching Claude Opus 4.8 / GPT 5.5; open weights; expensive and deep — the fit for the authoritative reviewer gate. (Forkast; Digit; Kimi-K3-vs-Pro comparison.)

## Alternatives considered

- **Keep routing implicit in chat and trailers.** Rejected — the arrangement has outgrown chat; the record can no longer answer "which model does what."
- **Make DeepSeek-V4-Pro the reviewer instead of Kimi** (it is the flagship and cheaper). Rejected — it is the same family as the driver, so it is not an independent reviewer; this is exactly what the independence rule forbids, and it would lose the cross-vendor catch Kimi provides.
- **Make DeepSeek-V4-Pro the sole second opinion** (drop GLM, escalate everything to Pro). Rejected — overkill for the small stuff and again same-family; GLM gives cheap *independent* diversity.
- **Three tiers only** (driver / second opinion / reviewer). Rejected — leaves no depth tier, so hard knots either burn the expensive reviewer or stall; the four-tier split keeps effort proportional to difficulty.
- **A single "best" model for everything.** Rejected — violates the cost budget and loses blind-spot diversity entirely.

## Consequences

- Routing is now inspectable and stable across sessions; a future agent reads this note before deciding which model to invoke.
- The independence rule makes the schema self-correcting: the only way to add a reviewer is a vendor the work's *author* is not in, so model families cannot silently share the same blind spot.
- Every new model the owner sets up has a decision procedure (which axis does it buy on — difficulty or independence?) instead of an ad-hoc guess.
- The four-tier cascade keeps the expensive gate from being the default path; it is reserved for substantive slices.
- Re-evaluation is a duty, not an option; the trigger keeps a moving-target field from silently freezing the table.
