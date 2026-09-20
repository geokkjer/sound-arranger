# Research: decision models (Laya, Jev) — what they are, and where they could fit here

> **Date:** 2026-09-20. **Scope:** the new "System 1 decision model" class — [Laya](https://huggingface.co/convaiinnovations/laya) (Convai Innovations, Apache-2.0) and Jev (TypeSafe's commercial original) — evaluated against this repo's *existing* arrangements: the [four-tier model co-work routing](../../.agents/notes/implemented/process/2026-08-27-model-co-work-routing.md) (driver / value pass / escalation / reviewer gate) and the [hub-and-spokes LLM seam](../../.agents/notes/proposed/architecture/2026-09-10-hub-and-spokes-and-the-llm-seam.md) (the Host API text format). **Method:** primary sources — model cards, the public typed-decisions benchmark and dataset, the provider API docs, crates.io metadata — plus this repo's own corpus measured for size (73 notes, 32 review archives, 125 commits). **Status:** research. The criteria it proposes are the [proposed note](../../.agents/notes/proposed/process/2026-09-20-decision-models-as-a-triage-tier.md).

## 0. Verdict on one screen

| Question | Answer |
|---|---|
| What *is* a "decision model"? | A small **non-generative encoder** that takes one short piece of state and answers **typed questions** — `choice` (pick an option key), `score` (ordinal), `noul` ("is it yes?", a probability) — returning **calibrated probabilities** in one forward pass. No text generation, nothing to parse, nothing to hallucinate. It is a *router / triage / guardrail*, not an assistant. |
| Jev? | TypeSafe's commercial "System One" API: text-only, 32K context, closed weights, ≈$0.042 / 1M tokens, 236–276 ms p50 (third-party measured), good on high-cardinality choice (>20–255 options). |
| Laya? | Convai Innovations' Apache-2.0 model in the same class, published 2026-09-18 and viral since (~3.4k★ in two days). **An independent implementation, not a release of Jev** — different company, same category (press coverage conflates them). Three checkpoints: English ModernBERT-large 421M/512 ctx, multilingual mmBERT-base 322M/1024 ctx, and a fine-tuned `typed-decisions` checkpoint. ~33 ms/question on a T4 (CPU 193–464 ms), 100+ languages. An independent benchmark ([JevBench v1.2](https://github.com/fstandhartinger/jevbench)) ranks it 70.1 against Jev's 75.4, with a Qwen3.5-4B-derived entrant at 74.7. |
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

### 1.2 Jev, Laya, and the rest of the field

Jev is **TypeSafe's** commercial original — "a structured decision model that returns typed answers (yes/no, choice, score) with calibrated probabilities instead of generated text… built for routing, classification, triage and other in-app decision points" ([AI/ML API docs](https://docs.aimlapi.com/api-references/decision-models/typesafe/jev)), text-only, 32K context, closed weights, ≈$0.042 per 1M input tokens.

Laya is **not** its open-source release. It comes from a different company, **Convai Innovations** (an Indian company whose product is an *offline* code-audit tool — see §4), which published a model in the same class and benchmarked it against Jev; the press has repeatedly described it as "Jev's open-source version", which the provenance does not support. The accurate description is an **independent Apache-2.0 competitor that adopts Jev's public wire interface** (`choice`/`score`/`noul`) — its author claims prior art (a paper from March 2025) and frames Jev as a relabelling, while TypeSafe discloses no architecture, parameter count or training data for Jev at all. Laya's card is careful about the comparison ("Jev figures are third-party published, never measured here").

And it is a **field, not a duopoly**: [JevBench v1.2](https://github.com/fstandhartinger/jevbench) (Benchmark Heaven's own, explicitly unaffiliated with TypeSafe) scores ten-plus "Jev-class" systems on Intelligence, Calibration, Speed and Cost — Jev 1.13.0 75.4, SemIf (Qwen3.5-4B) 74.7, djev (Maisa, diffusion-gemma) 74.3, **Laya 70.1**, open-alternative-jev 69.8, `system-one-open` 68.9, alongside `jeff` (GLiFormer 400M), GLiNER2 and an `openJev` implementation. Two things follow that matter more than any single leaderboard place: **a fine-tuned general small LLM is competitive with the purpose-built models**, and **small models are extremely sensitive to option order** — one entrant scored 21% instead of 72% on yes/no items when the same options were reversed.

### 1.3 What the model card *itself* says not to expect

This is the most useful part of the source material, because it is the vendor warning you off the naive adoption:

- "**Base checkpoints are near chance on typed-decisions zero-shot** — 0.362 here … against a 0.318 random and 0.461 majority-class baseline… **Laya is a fast base to specialise, not a zero-shot decision engine.**"
- "**Ships over-confident.** Refitting one temperature per (question type, option count) moves mean ECE **0.466 → 0.081** … Do this on your own data before trusting the probabilities."
- "The English checkpoint collapses on non-Latin scripts (**Khmer scores 0.000 accuracy at 0.952 confidence**). Because the model stays confident while being wrong, **confidence gating cannot save you**."
- High-cardinality `choice` degrades badly: with a fixed per-question option budget, 77 options get ~3–4 tokens each, and accuracy falls from ~0.87-class performance (Jev) to 0.425. Ordinal `score` is the weakest primitive.
- English-only on the root checkpoint; fine-tuning needs a GPU notebook (they ship a 2×T4 one) and a small labelled set — the reference evaluation is **400 cases / 2,000 decisions** over a public, *synthetic*, Apache-2.0 dataset.

## 2. The category, and the vocabulary this repo already has

"Decision model" (and TypeSafe's vendor-coined "System One") is a **new name for an old stack**: a calibrated, typed-question text classifier with a request-time option space. The novelty is the packaging — a request-time schema plus RL against proper scoring rules for calibration — not the components. Its neighbours, for orientation:

- **LLM routers** (semantic-router, RouteLLM, NotDiamond-style): pick a *model* per request, usually by embedding similarity to labelled exemplars. Laya/Jev generalise this to arbitrary typed questions, including yes/no and ordinals, with explicit calibration.
- **Cascades** (FrugalGPT-style): try the cheap model, escalate on low confidence. The decision-model class is the "cheap stage" of that pattern, with the confidence made explicit.
- **Cross-encoder classifiers / rerankers**: the same architecture family (encoder + scoring head); what is new here is that the label set is supplied per request rather than baked into the checkpoint.
- **Plain classifiers** (fine-tuned ModernBERT/BERT): one fixed label set, fixed at training time, no probabilities you should trust without temperature scaling.
- **Deterministic rules/policy**: exact, auditable, free, zero-variance — and *strictly better* than a probabilistic model for anything expressible as rules.
- **LLM prompting with constrained output**: generative and expensive, but it can *reason* and handle unseen decision shapes — which is why systems pair it as "System 2" behind a "System 1" filter.

(One name to file away as *unrelated*: the Decision Transformer is autoregressive, trajectory-conditioned offline RL; only the English word is shared.)

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

**There is a working precedent for exactly that pitch** — and it comes from the same company that published Laya. Convai Innovations sells **Nadhi Audit**: an offline, on-device code-security auditor for healthcare software ("Nothing leaves your machine… with a model we trained ourselves", OWASP/CWE findings written against HIPAA/DPDP/GDPR, ~$600/yr, macOS). Small specialised model + regulated domain + "the data cannot leave the building" is the shape of a product where a local System 1 is not a gimmick. It also reframes the category for us: these models ship *inside products*, they are not usually the product.

**And the benchmark evidence says the branded model is not the thing to buy.** The same week's independent work found a *frozen* small encoder plus a logistic-regression probe beating both a fine-tune and the hosted models — 0.933 vs Jev's 0.832 on Banking77, at ~9 ms and no per-call cost; a local encoder (GLiNER2.5) on an Apple M4 CPU answering in ~44 ms p50 where hosted Jev took 236–256 ms; and an open reproduction reaching usable accuracy by training a 150M encoder on 1,016 in-domain cases in ~25 minutes on a free Colab T4, concluding that "the binding constraint is 1,016 training cases, not model size or epochs". The industry-scale confirmation is Microsoft's **HyDRA** router: a ModernBERT-base (149M) classifier with four sigmoid heads, exported to ONNX, serving ~830K routing decisions a day at 99.98% availability with **86 ms median CPU inference** — and with head weights and threshold tunable at runtime, no retraining. So if the seam ever opens, the first move is a small local encoder (SetFit, or a frozen probe), not a branded decision model: **the contract is worth copying; the model is the cheap part.**

**The genuinely interesting project-specific idea.** The blocker for any bespoke decision model is labels — and this repo can *manufacture* them. The engine is deterministic and self-describing: it can generate valid and invalid command lists, render them, and observe exact outcomes (silence, clipping, log events, invariant violations). That yields an unlimited, exactly-labelled corpus for a command-level System 1 — the same "the tool generates its own training data" move, with labels better than any human annotation. (The public [`LocalLLaMA/typed-decisions`](https://huggingface.co/datasets/LocalLLaMA/typed-decisions) benchmark is the template: 400 synthetic cases, one state → several typed questions, explicitly framed as "does breaking a workflow into typed probabilistic decisions buy a better accuracy/calibration/latency trade-off than direct classification or prompting an LLM?" — the exact experiment we would want to run on one of our own decisions.)

**One modality warning.** Our most valuable decisions are about *audio* (does this bounce sound right? is this clip usable?). Laya and Jev are text-only. For audio decisions the right tools are audio embeddings/classifiers (or the multimodal LLM route already noted in the routing note's GLM evaluation trigger); a text decision model would only see metadata, and metadata is the least interesting part.

## 5. If we ever try it: the evaluation protocol

Whatever the decision, the method is fixed, and it is cheap to run:

1. **Write the rule first.** Define the decision as rules; measure agreement with the human/driver. Rules are the baseline any model must beat.
2. **Get an LLM ceiling.** Prompt a strong model on the same decisions with the same options — that is the accuracy ceiling a small model is chasing (Laya's card uses its "teacher" this way: the fine-tune *cleared* the teacher ceiling, 0.766 vs 0.735).
3. **Only then a small model**, fine-tuned on a few hundred labelled examples (the reference benchmark evaluates 400 cases / 2,000 decisions), with temperature scaling per (question type, option count) fitted on held-out data before any probability is trusted.
4. **Judge on calibration, not accuracy.** Report ECE/Brier alongside accuracy, and decide thresholds from the *cost asymmetry* of the actual decision (a missed gate is expensive; a needless escalation is cheap ⇒ threshold low). Calibrate **per question, not per model**: independent evaluation found the sign of miscalibration flipping by question type on the same inputs, probabilities quantized to 0.01 (with exact 0/1 values that temperature scaling cannot repair), and the confidence field unsafe to threshold on directly.
5. **Test distribution shift — and option order — explicitly.** The Khmer 0.000@0.952 example is the shift failure mode (confidently wrong defeats gating), and JevBench documents the other one: reversing the *same* yes/no options moved one entrant from 72% to 21%. Hold out a different slice and re-run with permuted options before trusting anything.
6. **Price the taxonomy.** Every new decision is a new question set and new labels; if decisions churn, the maintenance cost exceeds the inference saving.

Rust path, if it gets that far: there is no ready-made Laya runtime for Rust — the published runtimes are PyTorch/`transformers` (Python), plus community CoreML, MLX and ONNX exports (so the ONNX conversion need not be ours; if we do export it ourselves, mind the traps — below opset 18 the graph is silently invalid, and weights land in a sidecar `.onnx.data`). Running one in a Tauri app means executing encoder + custom head with `ort` (ONNX Runtime, MIT/Apache, 2.0.0-rc.13 — still an rc, x86-64-v3 floor, ships dylibs to sign), `rten` (pure Rust, MIT/Apache, 0.26.0) or `tract`, and re-implementing the option-marker scoring and the `choice`/`score`/`noul` heads if we convert ourselves (`candle` has a native ModernBERT, but not Laya's head; `tokenizers` needs `default-features = false` to avoid a C++ build). Practical constraints: weights are 0.6–1.7 GB, so ship them *outside* the installer (sidecar or first-run download); and **do not assume full int8** — Microsoft's production router reports that full INT8 quantization of ModernBERT "collapses predictions to a near-constant baseline", and that quantizing only the non-attention nodes buys ~40% size and ~8% latency, so quantize-and-validate per target CPU rather than trusting the usual 3–4×.

Honest numbers for sizing, and they are strongly **platform-dependent**: 86 ms median for a 149M classifier on production CPU (HyDRA, above) and ~100 ms to a few hundred ms for a 322–421M encoder on a desktop CPU, but ~5 ms on an Apple-Silicon Neural Engine and 33 ms on a T4. So the offline/local story is far better on Apple Silicon than on x86 CPU — worth knowing before designing around it. **And never on the audio thread.** A 512-sample block at 44.1 kHz is 11.6 ms, and every measured figure for this class is one to two orders of magnitude away on CPU (4–40 ms even on a GPU/ANE). Whatever a model decides here lives on the **control side**, feeding the log — the render path stays model-free, like every other decision in this engine.

## 6. Recommendation

1. **Do not add a decision model to the co-work loop.** It is the wrong tool for the wrong-shaped decision: too little context, too few decisions, unlabeled, asymmetric and silent failure modes. If triage friction is real, write the **rules** — a triage script over paths/class/diff-size that emits "mechanical → exempt" or "substantive → gate", and log its disagreements with the driver's judgement. That is a System 1 with exact semantics, and it costs an afternoon.
2. **Keep the category on the shelf for the product seam**, with a named trigger: a decision that is (i) repeated, (ii) semantic rather than syntactic, (iii) has a small, stable option set (<20), and (iv) can be labelled from engine-generated data or the session log. Below all four, rules or an LLM win.
3. **If it is ever opened, start from the interface, then from the cheap model — never from the brand.** The transferable idea is *typed questions with calibration* — `choice`/`score`/`noul` over a request-time option space, probabilities instead of labels. We can adopt that discipline with an LLM (prompt for distributions, check calibration on a held-out set) long before we adopt a 400M encoder; and if a small local model is then warranted, the evidence says start with a frozen probe or a SetFit-style classifier on ~1k labelled examples — independent work has that beating both fine-tunes and the hosted models on accuracy, latency and cost. The branded decision model is the last thing to try, not the first.
4. **Revisit on evidence, not fashion.** The *components* are old (encoders, cross-encoders, cascades, routers); what is new is the packaging — request-time schemas, calibration as a training objective, open weights — and it is moving fast. Vendor multipliers should be discounted by default: independent measurement is far more modest than the marketing (a single-digit-to-low-double-digit speed/cost advantage over LLMs, not the headline 100×-class numbers), and calibration — the alleged selling point — is the weakest independently measured axis. The re-evaluation trigger should be a *task* appearing, not a release. If one does appear on the product side, this memo plus the [proposed note](../../.agents/notes/proposed/process/2026-09-20-decision-models-as-a-triage-tier.md) are where the criteria live.
5. **Do not touch the routing note.** Any future change to *which models review what* is a new note superseding it — its own rule — and a decision model changes none of those rows.

## 7. Sources

**Laya:** [model card](https://huggingface.co/convaiinnovations/laya) (Apache-2.0; checkpoints, benchmarks, limits) · [typed-decisions checkpoint](https://huggingface.co/convaiinnovations/laya-typed-decisions) · [GitHub](https://github.com/NandhaKishorM/laya) (Apache-2.0, created 2026-09-18, ~3.4k★) · [HF demo space](https://huggingface.co/spaces/convaiinnovations/laya-demo) · PyPI `laya` 0.3.4 (Apache-2.0), community `laya-coreml` / `laya-mlx` runtimes.
**Jev:** [AI/ML API — TypeSafe Jev](https://docs.aimlapi.com/api-references/decision-models/typesafe/jev) (typed answers, 32K, commercial) · [Jev API key guide](https://apidog.com/blog/jev-api-key/) · third-party measurements cited by Laya: `AbdelStark/jev-benchmarks`, `nibzard/decision-model-benchmark`.
**Independent yardstick:** [JevBench v1.2](https://github.com/fstandhartinger/jevbench) (MIT; 534 decisions, four 25% axes, unaffiliated with TypeSafe) — rankings, the cost-per-1,000-decisions method, and the option-order footnote quoted above.
**Industry-scale precedent:** [HyDRA — Hybrid Dynamic Routing Architecture](https://arxiv.org/abs/2605.17106) (Microsoft; ModernBERT-base capability predictor, ONNX dynamic INT8 with attention excluded, ~830K routing decisions/day, 86 ms median CPU inference, runtime-tunable heads and threshold).
**The cheap-model counter-evidence:** the week's independent evaluations (frozen encoder + logistic probe at ~9 ms and 0.933 on Banking77; local GLiNER2.5 at ~44 ms p50 on an M4 CPU; a 150M encoder trained on 1,016 cases in ~25 min on a free T4) — surveyed in `AbdelStark/jev-benchmarks`, `nibzard/decision-model-benchmark` and the open reproductions they cite.
**The offline-precedent:** [Convai Innovations](https://convaiinnovations.com/) / Nadhi Audit (on-device code-security audit, "nothing leaves your machine").
**Benchmark/data:** [`LocalLLaMA/typed-decisions`](https://huggingface.co/datasets/LocalLLaMA/typed-decisions) (Apache-2.0, synthetic, <1k, four workflows).
**Rust inference:** crates.io metadata for `ort` 2.0.0-rc.13, `candle-core` 0.11.0, `tract` 0.23.7, `jammi-ai` 0.49.1 (an embeddable Rust inference/fine-tuning engine — not a Laya runtime).
**This repo's arrangements:** [model co-work routing](../../.agents/notes/implemented/process/2026-08-27-model-co-work-routing.md) · [hub-and-spokes LLM seam](../../.agents/notes/proposed/architecture/2026-09-10-hub-and-spokes-and-the-llm-seam.md) · corpus counts measured with `git rev-list`/`git log --format=%(trailers)` on 2026-09-20.

*Authored with deepseek-v4-flash · DeepSeek Harness, 2026-09-20.*
