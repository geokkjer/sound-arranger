# Agent Note: Umbrella-first — the platform is the goal; sound-arranger is the first profile

Status: proposed

## Problem

The project began as an ACID/DAW recreation with tape inspiration, and the research still calls the clip arranger "Phase 1 (the goal)" (RESEARCH.md §11) while the architecture (minimal core, everything-as-plugin) optimizes for a broader platform. The kimi review named this the sharpest strategic incoherence: "one of these is the real priority; the architecture review can't settle that" (`research/architecture/2026-08-15-kimi-review-minimal-core.md` §7.3). The release of deepseek-harness and Cordis reframed the project: the umbrella is not a byproduct of the arranger — it is the point. An unmade priority call keeps leaking into phase ordering, scope decisions, and how much plugin machinery is justified. This note locks the direction.

## Proposal

- **The platform is the goal.** A minimal core (clock · graph interpreter · session log · context plumbing — the [minimal-core note](2026-08-15-minimal-core-clock-graph-session-log.md)) with every capability as a plugin; products are assembled profiles. Spurred by the deepseek-harness/Cordis release: "everything is a plugin" demonstrated at production scale (RESEARCH.md §16).
- **The first profile is the clip-arranger ("sound-arranger")**: recorder + clip editor + soft mixer. Chosen because it stresses the substrate end-to-end — recording (media engine, writer, device-clock drift), editing (graph swap under load, edit-during-playback, the log), mixing and the realtime path (parameter smoothing, meters, bounce). Spike B exists precisely because this profile is the platform's stress test; shipping it first is the umbrella's validation, not a detour from it.
- **Offline/non-realtime processing splits into its own deferred profile — "sound sculptor" (working name).** CDP8 sidecar, PaulStretch, phase-vocoder stretch as `OfflineProcess` plugins with progress events. It stops being a tier of the arranger: sound sculpting (render-to-new-source work) is a distinct creative activity with its own workflow (process browser, job queue, auditioning). Unscheduled; no earlier than Phase 3, and only after the arranger profile and generators have exercised the seams.
- **Plugin-machinery budget rule (binding):** no composition machinery beyond what a phase-0 spike or a real second provider demands. This makes the kimi review's over-engineering warning (§2) testable instead of aspirational.
- **Naming:** the repo name "sound-arranger" names the first profile, not the platform. Rename candidates ("audio", "sound") are open — tracked as RESEARCH.md §14 risk 7; resolve before the first tag. No rename is executed by this note.
- **Second opinions:** plugin-architecture decisions get a DeepSeek review (proximity to cordiverse/paper and dsh — the paradigm's own ecosystem); the kimi-cli review (`research/architecture/2026-08-15-kimi-review-minimal-core.md`) is the precedent for archiving external reviews verbatim with a disposition header.

**Superseded in part (2026-09-21):** the *sound-sculptor* profile still stands as a separate
profile, but its artifacts are external programs invoked and imported, not CDP8/`OfflineProcess`
sidecars we ship ([note](2026-09-21-external-programs-not-sidecars.md)).

## Alternatives considered

- **Arranger-first, umbrella as accidental byproduct** — the original framing. Rejected: the core/seams decisions are already platform decisions, and leaving the priority ambiguous produced the §7.3 incoherence (docs calling the arranger "the goal" while phases optimize for the framework).
- **Umbrella-first with a smaller smoke profile** — a minimal demo profile would validate the paradigm more cheaply, but it would not stress streaming, the recording writer, or drift; the arranger-as-first-profile *is* Spike B promoted to product. Rejected: a platform validated only on toys hardens the wrong core.
- **Keep the ambiguity (arranger is "the goal", umbrella is the architecture)** — rejected: phase ordering and the plugin-machinery budget need one authority; every future "is this in scope?" call would re-litigate it.
- **Keep offline processing as an arranger tier (Phase 3, status quo)** — rejected: render-to-new-source sculpting has its own workflow, and keeping it inside the arranger re-couples the profile to CDP8/CMake sidecar concerns it does not need. Deferring it also shrinks the arranger's phases to pure realtime work.

## Acceptance criteria

- Phase 1 ships the clip-arranger profile (recorder + clip editor + soft mixer) with no offline-processing UI in it.
- The `OfflineProcess` seam is exercised only when sound-sculptor work starts (the trait definition may exist earlier where free); the first sound-sculptor change adds its profile entry and cites this note.
- Every addition of composition machinery (loader rows, patches, ctx keys beyond core) cites — in the committing note — the spike or the real second provider that demands it.
- The name question is decided (rename or explicit keep) before the first tag/release.

## Risks

- **The framework eats the product** (kimi review §2) — the classic failure: composition machinery becomes the project and no profile ships. Mitigation: the budget rule above, plus Phase 1 being a profile, not more kernel.
- **Sound-sculptor starvation** — "deferred" can mean "never"; CDP8 integration is where the `OfflineProcess` seam gets its proof. Accepted: it was always Phase 3+, and the seam definition is cheap to keep honest in the meantime.
- **Rename churn** — repo path, the `~/Projects/music` symlink view, back-references from the theory corpus. Mitigation: decide once, at a quiet moment before external surface area grows.
- **Reviewer monoculture** — leaning on DeepSeek for plugin-architecture calls imports the paradigm's own blind spots (the kimi review exists precisely because a generic plugin lens missed realtime-audio realities). Mitigation: keep pairing paradigm-adjacent reviews with domain (realtime-audio) reviews.
