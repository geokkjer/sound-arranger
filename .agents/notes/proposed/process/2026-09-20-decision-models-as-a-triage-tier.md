# Agent Note: Learnt decision models as a triage tier — criteria, and the verdict not to adopt one now

Status: proposed

## Problem

A new class of small "System 1 decision models" arrived in September 2026 — TypeSafe's commercial **Jev** and **Convai Innovations'** open-weights **Laya** (Apache-2.0, 2026-09-18; independent implementations in the same class from different companies, not a release of one another). Both take one short piece of state (text, ticket, JSON) plus **typed questions** (`choice`, `score`, `noul`) and return **calibrated probabilities** in a single forward pass, without generating text ([Laya model card](https://huggingface.co/convaiinnovations/laya)).

The question they raise here is positional, and the repo cannot answer it from the record: the project already runs a **four-tier co-work routing** (driver / value pass / escalation / reviewer gate, [note](../../implemented/process/2026-08-27-model-co-work-routing.md)) and already has an **LLM seam** in the product (the Host API text format, [note](../architecture/2026-09-10-hub-and-spokes-and-the-llm-seam.md)). Is a decision model a new tier in the first, a new component behind the second, or neither — and if it is neither today, what would have to change for it to be worth revisiting? Without a written answer, each new release invites the same ad-hoc debate.

The full evaluation, with numbers and sources, is the [research memo](../../../../research/architecture/2026-09-20-decision-models-laya-jev-and-the-llm-seam.md).

## Proposal

**Record the criteria and do not adopt a learnt decision model now — for either the development loop or the product seam. Re-open on a named trigger.**

1. **It is not a review tier, and must never be added as one.** Review is generative, adversarial and cross-vendor by the routing note's independence rule; a classifier produces no findings and, being one vendor's weights, carries no independence. Nothing about the existing four-tier table changes.
2. **Triage-by-model fails on shape, not on quality** — measured against this repo (125 commits, ~2/day; 32 review archives; 73 notes):
   - *Context:* the English checkpoint's state budget is 512 tokens (~320 after the option budget); a diff plus its relevant invariants do not fit, and summarising first re-introduces the LLM call the model was meant to save.
   - *Hidden knowledge:* routing depends on invariants (byte-identical replay, zero-alloc render, PDC, supersede-don't-rewrite) that live in notes, not in the diff text.
   - *Volume:* a handful of routing decisions a day is no volume to amortise a model, its labels, and its maintenance.
   - *Asymmetry:* a false "mechanical → skip the gate" is a silent, expensive error; a false "escalate" is a few cents. A probabilistic router adds a new silent failure mode to a process designed to catch silent failures.
   - *Labels:* no recorded ground truth for "was this substantive"; 32 archives is not a training set.
   - *And the zero-shot base is not usable:* Laya's own card reports the untuned checkpoint at 0.362 against a 0.461 majority-class baseline — "a fast base to specialise, not a zero-shot decision engine".
3. **If triage friction is real, encode the policy as rules, not a model.** A script over file paths, change class and diff size emitting `mechanical → exempt` / `substantive → gate` is exact, auditable, free, and consistent with what the repo already does in `scripts/verify-agent-notes.mjs` and the pre-commit hook. The learnt model would be a probabilistic *downgrade* of a decision we can already compute.
4. **The product seam is where a local System 1 could eventually belong — with a four-condition trigger.** Adopt one only when a decision is all of: (i) **repeated**, (ii) **semantic** rather than syntactic (the engine's closed registry and refusals already decide everything syntactic, exactly), (iii) has a **small, stable option set** (<20 — high-cardinality choice degrades sharply), and (iv) is **labelable** from engine-generated data or the session log. Below all four, deterministic rules or an LLM win. The justification must be **offline/privacy/cost-at-volume**, never speed: the hub-and-spokes note removes latency as a design constraint.
5. **Adopt the interface before the model.** The transferable idea is typed questions with calibrated probabilities (`choice`/`score`/`noul` over a request-time option space), not a 421M encoder. Asking an LLM for distributions and checking calibration on held-out data is a zero-dependency way to learn whether a small model would later pay. If one ever does, the engine can *manufacture* its labels — valid and invalid command lists, rendered, with exact observed outcomes — which is a better corpus than any human annotation.

## Alternatives considered

- **Add it as a cheap pre-filter in front of the reviewer gate** — rejected: it is not independent (single-vendor weights), it cannot generate findings, and a false negative removes exactly the check the gate exists to provide. Independence, not cost, is what the gate buys.
- **Adopt it as a dev-loop router now and measure later** — rejected: there is no volume to measure and no labels to score against; the experiment would cost more than the decision it automates, and a mis-tuned threshold could quietly suppress reviews.
- **Use it in the product as a guardrail on LLM-authored command lists** — rejected *for syntactic* validity (the engine's parser and refusals are exact and already exist; a probabilistic check is strictly worse), and premature for semantic validity (that is the four-condition trigger, not met).
- **Run it in-process for audio decisions** — rejected on modality: Laya and Jev are text-only, while the decisions worth automating here are about audio; that line is audio embeddings/classifiers or a multimodal LLM, not a text decision model.
- **Fine-tune one on the repo's own history (commits, reviews, notes)** — rejected as a first move: the corpus is ~125 commits and 32 archives with confounded labels, and the decision it would predict is the one the driver already makes conservatively at negligible cost.
- **Do nothing and keep the question in chat** — rejected: the packaging is new and moving fast (open weights, request-time schemas, an independent benchmark and a public dataset all within a week), so the same question will recur with the next release; the criteria belong in the record, not in a conversation.

## Acceptance criteria

- The repo's position is inspectable in one place: decision models are not a review tier, are not adopted for dev-loop triage, and have a four-condition trigger for the product seam (this note), with the evidence and numbers in the [research memo](../../../../research/architecture/2026-09-20-decision-models-laya-jev-and-the-llm-seam.md).
- The [routing note](../../implemented/process/2026-08-27-model-co-work-routing.md) is unchanged; if a decision model is ever adopted, that change supersedes it with a fresh note rather than editing it.
- Any future proposal names the concrete decision it would make, its option set, its label source, and its rules/LLM baselines — the three-baseline protocol in §5 of the memo.

## Risks

- **Fashion risk both ways.** Dismissing a fast-moving class on day three, or adopting it because it trended, are the same error; the trigger is a task, not a release, and this note should be re-read when one appears.
- **The rules-first recommendation is not free.** A triage script must be maintained as the note tree and file layout evolve; if it silently rots, it becomes a false assurance. It should log its own disagreements with the driver's judgement rather than gate anything automatically at first.
- **Calibration is a trap for the unwary.** Laya's card documents over-confidence (ECE 0.466 → 0.081 only after temperature fitting) and a catastrophic shift case — "Khmer scores 0.000 accuracy at 0.952 confidence" — so any future adoption must fit temperatures on our data and test a shifted slice before trusting a threshold.
- **Modality mismatch may be permanent.** If the decisions worth automating here turn out to be mostly audio, this whole class stays irrelevant to the product and the note's shelf life is short — which is a fine outcome, and the trigger conditions make it visible.

*Authored with deepseek-v4-flash · DeepSeek Harness, 2026-09-20.*
