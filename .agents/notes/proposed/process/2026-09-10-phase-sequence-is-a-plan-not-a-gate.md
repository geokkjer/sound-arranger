# Agent Note: The phase sequence is a plan, not a gate — Phase 3 is a suggestion until Phase 2 is signed off

Status: proposed

## Problem

`RESEARCH.md` §11 presents Phases 0–5 as a numbered sequence, and several notes then use those
numbers as **prohibitions**: "the OfflineProcess seam is exercised only when sound-sculptor work
starts… no earlier than Phase 3"; "Tier 2 belongs to the deferred sound-sculptor profile"; "the
OfflineProcess sidecar job moves to the sound-sculptor profile". Nothing distinguishes a
**sequence** (this is a sensible order to build things in) from a **gate** (you may not start X
until Y ships).

That conflation has two costs.

**A gate binds a plan to a moment that has passed.** The phase list was drawn while the project
was pre-code. Phase 3 is defined as "real-time effects: `fundsp` effect plugins + dub sends" —
but `crates/engine/src/plugins/fundsp_synth.rs` already exists from the 2026-08-30 soft-synth
spike, so part of Phase 3's substrate is proven ahead of its phase. A numbered gate invites
building a feature *because it is Phase 2* rather than because it is wanted.

**It makes scoping decisions look like permissions.** The sound-sculptor deferral is a statement
about **which profile owns a workflow** — not a claim that CDP8 cannot be built. The
`OfflineProcess` trait is already defined, and a *spoke* needs no profile change at all (the
[hub-and-spokes note](../architecture/2026-09-10-hub-and-spokes-and-the-llm-seam.md)). Read as a gate, the
deferral forbids something it was never about.

## Proposal

- **The phase list is a planning aid and a shared vocabulary for scope, not an authorisation
  gate.** It states an intended order. It does not forbid starting something earlier, and it
  does not require resequencing to be justified by failure.
- **The transition into Phase 3 is an explicit decision**, taken when Phase 2 is in a state the
  author is *happy with* — not an automatic consequence of Phase 2's bullet list being finished.
  "Happy with" is a judgement about whether the tooling produces work worth keeping, and it is
  the judgement that should decide the transition. **Phase 3 is a suggestion until that decision
  is taken.**
- **Phase 2's completion is not its feature list.** Phase 2 as written (chord-progression plugin,
  improv plugin, decoupled pitch and rhythm, external synth providers) is a *means* toward an
  end: a generative performer that produces runs worth arranging. If the arranger already
  produces music the author wants to keep using a subset of that, then Phase 2 is done for this
  purpose and the remainder is optional depth.
- **Deferrals must say whether they rest on scope or dependency.** A *dependency* deferral is a
  real gate (X cannot work until Y exists). A *scope* deferral is not (the capability could be
  built; the question is which profile surfaces it). Notes citing a phase number should say
  which kind they mean.
- **Record the Phase 2 → Phase 3 decision in its own note when it happens**, stating what Phase 2
  actually delivered, what is being carried forward, and what is being deferred. That note — not
  the phase number — is the authority.

## Alternatives considered

- **Keep the numbered gates as written** — rejected: a plan drawn pre-code should not bind a
  project post-code, and "it is Phase 2, so build it" is exactly the failure mode the
  plugin-machinery budget rule in the [umbrella-first note](../architecture/2026-08-15-umbrella-first-product-direction.md)
  was written to prevent, applied to phases instead of plugins.
- **Delete the phase list entirely** — rejected: it is genuinely useful scope vocabulary, several
  notes cite it, and §11 is the shared reference for what "Phase 1" means. The problem is its
  *force*, not its existence.
- **Define Phase 2 completion as an exhaustive checklist** — rejected: it converts the transition
  into a formality and removes the only judgement that matters (is the tooling producing work we
  want?). Checklists measure progress, not readiness.
- **Treat every deferral as a gate** — rejected: it conflates scoping with dependency, and in the
  sculptor's case it would forbid building a sidecar that touches no profile.
- **Let the transition be implicit** — rejected: an implicit transition is how a phase list
  becomes a gate by default. The decision must be made out loud, even if it is "carry on as
  planned".

## Acceptance criteria

- `RESEARCH.md` §11 distinguishes *sequence* from *gate*, and the sound-sculptor entry states
  which kind of deferral it is (scope).
- The Phase 2 → Phase 3 transition is recorded in its own note at the time, naming what Phase 2
  delivered and what is deferred.
- No new note uses a phase number as a prohibition without saying whether it is scope or
  dependency.
- The sculptor's decision point is stated explicitly: **Phase 2 sign-off**, not a phase number.

## Risks

- **"We'll decide later" dissolves the plan.** If every boundary is reopened, ordering value is
  lost and phase references become noise. Mitigation: the order keeps its force as an *intended*
  sequence; only the *transition decision* is deferred — and it must actually be taken, in a
  note, rather than rolled forward silently.
- **Sculptor starvation gets worse.** "Decide at sign-off" can become "never", which is the risk
  the [umbrella-first note](../architecture/2026-08-15-umbrella-first-product-direction.md) already names.
  Mitigation: the sculptor has a named decision point; if it slips at that point it must be
  revisited rather than renewed.
- **Existing phase references read as stale.** Several notes cite "Phase 2+" or "Phase 3+" as
  justification. This note changes their *force* (advisory, not prohibitive) without changing
  their content, so they need not be rewritten — but they will read differently, and a future
  agent may mistake the change for inconsistency. Mitigation: cite this note from §11.
- **Judgement replaces a test.** "A state we are happy with" is not machine-checkable. Accepted
  deliberately: the alternative is a checklist that declares Phase 2 done while the music is
  still not worth arranging.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-10.
