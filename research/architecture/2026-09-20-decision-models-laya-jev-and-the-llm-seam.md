# Research: decision models (Laya, Jev) — what they are, and where they could fit here

> **Date:** 2026-09-20. **Scope:** the new "System 1 decision model" class — [Laya](https://huggingface.co/convaiinnovations/laya) (Convai Innovations, Apache-2.0) and Jev (TypeSafe's commercial original) — evaluated against this repo's *existing* arrangements: the [four-tier model co-work routing](../../.agents/notes/implemented/process/2026-08-27-model-co-work-routing.md) (driver / value pass / escalation / reviewer gate) and the [hub-and-spokes LLM seam](../../.agents/notes/proposed/architecture/2026-09-10-hub-and-spokes-and-the-llm-seam.md) (the Host API text format). **Method:** primary sources — model cards, the public typed-decisions benchmark and dataset, the provider API docs, crates.io metadata — plus this repo's own corpus measured for size (73 notes, 32 review archives, 125 commits). **Status:** research. The criteria it proposes are the [proposed note](../../.agents/notes/proposed/process/2026-09-20-decision-models-as-a-triage-tier.md).

## 0. Verdict on one screen

| Question | Answer |
|---|---|
| What *is* a "decision model"? | A small **non-generative encoder** that takes one short piece of state and answers **typed questions** — `choice` (pick an option key), `score` (ordinal), `noul` ("is it yes?", a probability) — returning **calibrated probabilities** in one forward pass. No text generation, nothing to parse, nothing to hallucinate. It is a *router / triage / guardrail*, not an assistant. |
| Jev? | TypeSafe's commercial "System One" API: text-only, 32K context, closed weights, ≈$0.042 / 1M tokens, 236–276 ms p50 (third-party measured), good on high-cardinality choice (>20–255 options). |
| Laya? | Convai Innovations' Apache-2.0 open answer to it, published 2026-09-18 and viral since (~3.4k★ in two days). Three checkpoints: English ModernBERT-large 421M/512 ctx, multilingual mmBERT-base 322M/1024 ctx, and a fine-tuned `typed-decisions` checkpoint. ~33 ms/question on a T4 (CPU 193–464 ms), 100+ languages. |
| Use it to replace a review model? | **No — category error.** Review is *generative, adversarial and cross-vendor by rule*; a classifier produces no findings, and as a single vendor's model it carries no independence. It is not a tier in that table at all. |
| Use it as a dev-loop router (a tier −1)? | **Not now.** 512-token state vs. whole diffs; the routing decision depends on project invariants that are not in the diff; ~2 commits/day gives nothing to amortise; a wrong "skip the gate" is silent and expensive; and Laya's own card says the untuned base is *below the majority-class baseline* on typed decisions. |
| Anywhere it genuinely helps? | **The product's LLM seam**, later: a local, offline System 1 for *bounded, repeated, semantic* decisions (pool triage, session-log intent, "which of these N actions"), where the appeal is privacy/offline/cost — not speed, because the authorial loop has already declared latency a non-constraint. |
| Do the near-term wins need a model? | Mostly no. The engine is already a **deterministic decision oracle** for everything syntactic (the closed registry, the refusals, the log, `scripts/verify-agent-notes.mjs`). A learnt model only earns a place where a decision is semantic, repeated, high-volume **and** labelable. |

## 1. What Laya and Jev actually are

### 1.1 The interface is the interesting part, not the language model

Both are the same shape: **state in, typed questions in, typed answers with probabilities out.** Laya's card is explicit:

> "Multilingual, non-autoregressive System 1 decision model. Give it a **state** (text, email, ticket, or JSON) and **typed questions**; it returns typed answers with mathematically calibrated probabilities in a single forward pass (~33 ms) across 100+ languages. Trained with reinforcement learning against strictly proper scoring rules (**RLCD**), so reporting honest probabilities is the only way to maximise reward. It never generates text, so there is nothing to parse and nothing to hallucinate."

The three primitives, taken from the model card's own example:

```python
questions = {
    "department":      {"type": "choice", "criteria": {"billing": "...", "technical": "...", "sales": "...", "other": "..."}},
    "urgency":         {"type": "score",  "criteria": ["not urgent", "soon", "critical deadline or blocking issue"]},
    "churn_risk":      {"type": "noul",   "instructions": "Does the user threaten to cancel or leave?"},
}
res = agent.predict(state, questions)   # one forward pass; every answer is a distribution
```

**The answer space is defined at request time** — a `choice` question's options are scored at their own `[MASK]` tokens and softmaxed — so *a new decision schema needs no retraining*. That is the property that makes this class different from an ordinary fine-tuned classifier: the model is a general "ask it typed questions about this text" engine, and only the *question* is new.

Technically: ModernBERT-large (395M, fully fine-tuned) plus a decision head trained from scratch (2 transformer layers, an option-marker scorer, an act/escalate head) = 421M. RLCD = REINFORCE/GRPO-style updates where the reward is a strictly proper scoring rule (log + spherical, plus ranked probability score for ordinals) — so honesty about uncertainty is the reward-maximising policy.

### 1.2 Jev, and the relationship

Jev is **TypeSafe's** commercial original — "a structured decision model that returns typed answers (yes/no, choice, score) with calibrated probabilities instead of generated text… built for routing, classification, triage and other in-app decision points" ([AI/ML API docs](https://docs.aimlapi.com/api-references/decision-models/typesafe/jev)). Laya is the **open-weights answer**: Apache-2.0, self-hostable, and (by its own published benchmarks) ahead on accuracy, calibration and latency, behind on context (512/1024 vs Jev's 32K) and on high-cardinality choice sets. Its card is careful to label the Jev comparison "third-party published, never measured here".

### 1.3 What the model card *itself* says not to expect

This is the most useful part of the source material, because it is the vendor warning you off the naive adoption:

- "**Base checkpoints are near chance on typed-decisions zero-shot** — 0.362 here … against a 0.318 random and 0.461 majority-class baseline… **Laya is a fast base to specialise, not a zero-shot decision engine.**"
- "**Ships over-confident.** Refitting one temperature per (question type, option count) moves mean ECE **0.466 → 0.081** … Do this on your own data before trusting the probabilities."
- "The English checkpoint collapses on non-Latin scripts (**Khmer scores 0.000 accuracy at 0.952 confidence**). Because the model stays confident while being wrong, **confidence gating cannot save you**."
- High-cardinality `choice` degrades badly: with a fixed per-question option budget, 77 options get ~3–4 tokens each, and accuracy falls from ~0.87-class performance (Jev) to 0.425. Ordinal `score` is the weakest primitive.
- English-only on the root checkpoint; fine-tuning needs a GPU notebook (they ship a 2×T4 one) and a small labelled set — the reference evaluation is **400 cases / 2,000 decisions** over a public, *synthetic*, Apache-2.0 dataset.

## 2. The category, and the vocabulary this repo already has

"Decision model" is the marketing name for a **calibrated, typed-question text classifier with a request-time option space**. Its neighbours, for orientation:

- **LLM routers** (semantic-router, RouteLLM, NotDiamond-style): pick a *model* per request, usually by embedding similarity to labelled exemplars. Laya/Jev generalise this to arbitrary typed questions, including yes/no and ordinals, with explicit calibration.
- **Plain classifiers** (fine-tuned ModernBERT/BERT): one fixed label set, fixed at training time, no probabilities you should trust without temperature scaling. Laya's contribution is the *request-time schema* plus RL-for-calibration.
- **Deterministic rules/policy**: exact, auditable, free, zero-variance — and *strictly better* than a probabilistic model for anything expressible as rules.
- **LLM prompting with constrained output**: generative and expensive, but it can *reason* and handle unseen decision shapes — which is why systems pair it as "System 2" behind a "System 1" filter.

Terminology caution: in this repo "decision" already means the *record* (Agent Notes hold decisions; the routing table is a written decision procedure). A learnt decision model is a third thing: a **policy component**, not a record and not a reviewer.

## 3. Fit A — the development loop (the review arrangement)

The question "we already use other models for review — should a decision model join that?" answers itself once the roles are separated:

| Role | What it does | Can a Laya-class model do it? |
|---|---|---|
| Reviewer gate (Kimi) | Generate adversarial findings on a substantive slice | **No.** Non-generative by construction. |
| Value pass (GLM) | Cheap independent second opinion | **No** — and it would violate the independence rule, being one vendor's weights rather than an outside opinion. |
| Escalation (Pro) | Depth on a hard knot | No. |
| **Triage / dispatch** | "Which tier does this change belong in? Does it need a note? Does it touch the realtime path?" | **Shape-wise yes, practically no** — see below. |

Why triage-by-model fails *here*, concretely:

1. **Context.** The English checkpoint's state budget is 512 tokens (~320 after the option budget). A typical commit diff is larger, and the parts that matter (does it touch the render path? does it change a wire format?) are not the parts that dominate the text. Summarising the diff first is itself an LLM call — you have re-invented the driver you were trying to save.
2. **Hidden knowledge.** The routing rules reference invariants (byte-identical replay, zero-allocation render, PDC, "never rewrite a decision, supersede it"). Those are in notes, not in the diff. A classifier keyed on surface features would be guessing.
3. **Volume.** Measured: 125 commits, ~2/day, 32 review archives, 73 notes. The routing decision happens a handful of times a day. There is no volume to amortise a model, its training set, or its maintenance.
4. **Asymmetric, silent errors.** A false "mechanical, skip the gate" ships a bug; a false "escalate" costs a few cents. The existing LLM driver errs conservative for free — a probabilistic router adds a *new silent failure mode* to a process whose entire purpose is catching silent failures.
5. **Labels.** Ground truth would be "was this substantive?", which nobody recorded; the confound is that *which* model reviewed what reflects the trial schedule, not a judgement. 32 archives is not a training set.
6. **It is already solved cheaply.** The policy is expressible as rules over paths/classes/diff size (the repo already does exactly this shape of thing in `scripts/verify-agent-notes.mjs` and the pre-commit hook). **Encode the policy, not a model.** A rules-based triage is exact, auditable, free and instant — and it is the System 1 the process actually lacks.

## 4. Fit B — the product's LLM seam (the more promising direction)

Here the shape matches much better, with caveats.

**What the seam already is.** The Host API is a versioned, closed-vocabulary **text command language** whose refusals are the teaching signal and whose log is memory ([hub-and-spokes note](../../.agents/notes/proposed/architecture/2026-09-10-hub-and-spokes-and-the-llm-seam.md)). That is a *typed decision space*: `mount`, `patch`, `set_param`, `splice`, `bounce`… over a closed registry, with exact refusals.

**The honest catch:** for everything *syntactic*, the engine is already a perfect, deterministic decision oracle. A probabilistic classifier can only be worse at "is this a legal command list?" — it cannot beat a parser. A learnt System 1 earns a place only for decisions the engine *cannot* make:

- **semantic/similarity**: "which clip in the pool does this sentence describe?", "is this take a variation of that one?"
- **intent with ambiguity**: "the user said 'tighter' — which of these N parameters did they mean?"
- **taste/curation**: "is this run worth keeping?" — the highest-value decision and the hardest to label, because the label is the composer's taste and exists only a few dozen times per session.

**Why offline matters, not speed.** The hub-and-spokes note explicitly removes latency as a design constraint ("batching, thinking, revising and re-running are the normal mode"). So the case for a local model cannot be "33 ms instead of 3 s". It has to be: **the app ships an assistant that needs no API key, works on a plane, keeps the session log on the machine, and costs nothing at volume** — plus the deferred Raspberry-Pi/embedded phase, where a cloud round-trip may not exist at all. That is a product decision, not an efficiency one, and it should be argued that way.

**The genuinely interesting project-specific idea.** The blocker for any bespoke decision model is labels — and this repo can *manufacture* them. The engine is deterministic and self-describing: it can generate valid and invalid command lists, render them, and observe exact outcomes (silence, clipping, log events, invariant violations). That yields an unlimited, exactly-labelled corpus for a command-level System 1 — the same "the tool generates its own training data" move, with labels better than any human annotation. (The public [`LocalLLaMA/typed-decisions`](https://huggingface.co/datasets/LocalLLaMA/typed-decisions) benchmark is the template: 400 synthetic cases, one state → several typed questions, explicitly framed as "does breaking a workflow into typed probabilistic decisions buy a better accuracy/calibration/latency trade-off than direct classification or prompting an LLM?" — the exact experiment we would want to run on one of our own decisions.)

**One modality warning.** Our most valuable decisions are about *audio* (does this bounce sound right? is this clip usable?). Laya and Jev are text-only. For audio decisions the right tools are audio embeddings/classifiers (or the multimodal LLM route already noted in the routing note's GLM evaluation trigger); a text decision model would only see metadata, and metadata is the least interesting part.

## 5. If we ever try it: the evaluation protocol

Whatever the decision, the method is fixed, and it is cheap to run:

1. **Write the rule first.** Define the decision as rules; measure agreement with the human/driver. Rules are the baseline any model must beat.
2. **Get an LLM ceiling.** Prompt a strong model on the same decisions with the same options — that is the accuracy ceiling a small model is chasing (Laya's card uses its "teacher" this way: the fine-tune *cleared* the teacher ceiling, 0.766 vs 0.735).
3. **Only then a small model**, fine-tuned on a few hundred labelled examples (the reference benchmark evaluates 400 cases / 2,000 decisions), with temperature scaling per (question type, option count) fitted on held-out data before any probability is trusted.
4. **Judge on calibration, not accuracy.** Report ECE/Brier alongside accuracy, and decide thresholds from the *cost asymmetry* of the actual decision (a missed gate is expensive; a needless escalation is cheap ⇒ threshold low).
5. **Test distribution shift explicitly** — the Khmer 0.000@0.952 example is the failure mode; a model that is confidently wrong defeats confidence gating, so hold out a genuinely different slice (e.g. a diff class never seen in training).
6. **Price the taxonomy.** Every new decision is a new question set and new labels; if decisions churn, the maintenance cost exceeds the inference saving.

Rust path, if it gets that far: there is no ready-made Laya runtime for Rust — the published runtimes are PyTorch/`transformers` (Python), plus community CoreML and MLX wrappers. Bringing it into a Tauri app means exporting the encoder + decision head to ONNX and running it with `ort` (ONNX Runtime, MIT/Apache, v2.0.0-rc.13) or `candle`/`tract`, and re-implementing the option-marker scoring and the `choice`/`score`/`noul` heads — real work, not a `cargo add`. Weight size is ~650–810 MB, CPU inference 193–464 ms per request per the card.

## 6. Recommendation

1. **Do not add a decision model to the co-work loop.** It is the wrong tool for the wrong-shaped decision: too little context, too few decisions, unlabeled, asymmetric and silent failure modes. If triage friction is real, write the **rules** — a triage script over paths/class/diff-size that emits "mechanical → exempt" or "substantive → gate", and log its disagreements with the driver's judgement. That is a System 1 with exact semantics, and it costs an afternoon.
2. **Keep the category on the shelf for the product seam**, with a named trigger: a decision that is (i) repeated, (ii) semantic rather than syntactic, (iii) has a small, stable option set (<20), and (iv) can be labelled from engine-generated data or the session log. Below all four, rules or an LLM win.
3. **If it is ever opened, start from the interface, not the model.** The transferable idea is *typed questions with calibration* — `choice`/`score`/`noul` over a request-time option space, probabilities instead of labels. We can adopt that discipline with an LLM (prompt for distributions, check calibration on a held-out set) long before we adopt a 400M encoder, and the results will tell us whether a small model is worth it.
4. **Revisit on evidence, not fashion.** This class is three days old and moving fast (Apache-2.0 weights, 100+ languages, request-time schemas, a public benchmark and dataset). The re-evaluation trigger should be a *task* appearing, not a new release. If one does appear on the product side, this memo plus the [proposed note](../../.agents/notes/proposed/process/2026-09-20-decision-models-as-a-triage-tier.md) are where the criteria live.
5. **Do not touch the routing note.** Any future change to *which models review what* is a new note superseding it — its own rule — and a decision model changes none of those rows.

## 7. Sources

**Laya:** [model card](https://huggingface.co/convaiinnovations/laya) (Apache-2.0; checkpoints, benchmarks, limits) · [typed-decisions checkpoint](https://huggingface.co/convaiinnovations/laya-typed-decisions) · [GitHub](https://github.com/NandhaKishorM/laya) (Apache-2.0, created 2026-09-18, ~3.4k★) · [HF demo space](https://huggingface.co/spaces/convaiinnovations/laya-demo) · PyPI `laya`, community `laya-coreml` / `laya-mlx` runtimes.
**Jev:** [AI/ML API — TypeSafe Jev](https://docs.aimlapi.com/api-references/decision-models/typesafe/jev) (typed answers, 32K, commercial) · [Jev API key guide](https://apidog.com/blog/jev-api-key/) · third-party measurements cited by Laya: `AbdelStark/jev-benchmarks`, `nibzard/decision-model-benchmark`.
**Benchmark/data:** [`LocalLLaMA/typed-decisions`](https://huggingface.co/datasets/LocalLLaMA/typed-decisions) (Apache-2.0, synthetic, <1k, four workflows) · [`fstandhartinger/jevbench`](https://github.com/fstandhartinger/jevbench) (MIT, "a benchmark for Jev-class typed decision models").
**Rust inference:** crates.io metadata for `ort` 2.0.0-rc.13, `candle-core` 0.11.0, `tract` 0.23.7, `jammi-ai` 0.49.1 (an embeddable Rust inference/fine-tuning engine — not a Laya runtime).
**This repo's arrangements:** [model co-work routing](../../.agents/notes/implemented/process/2026-08-27-model-co-work-routing.md) · [hub-and-spokes LLM seam](../../.agents/notes/proposed/architecture/2026-09-10-hub-and-spokes-and-the-llm-seam.md) · corpus counts measured with `git rev-list`/`git log --format=%(trailers)` on 2026-09-20.

*Authored with deepseek-v4-flash · DeepSeek Harness, 2026-09-20.*
