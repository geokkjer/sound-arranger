# Agent Note: Control→render handoff — the flush pump, parked ops, and the concurrent-era trigger

Status: implemented

## Problem

Every control mutation reaches audio through the scheduling queue (`validate → log → schedule`), but *where* scheduled events apply was decided ad hoc per phase: most kinds apply on the render call stack, arrangement ops apply only on the control side (`flush_scheduled`) because handlers reconcile reader threads and file I/O. That split had two defects. First, a host that rendered without flushing sent arrangement ops down the render stack, where they were **popped and dropped** in release while remaining logged — replay would re-schedule and eventually apply them, so live output and a replay of the same log could silently diverge (found in the 2026-08-27 ZCode review pass; the debug assert caught it only in dev builds). Second, no note owned the handoff as such: the lock-free channel was promised "in Phase 1" by the spike-B and P1.2 notes, survived three shipped phases, and nobody could say what mechanism was normative *now* versus aspirational — while shell/UI work was about to make a GUI the first genuinely concurrent controller.

## Decision

There is **one mechanism**: every control→render mutation goes through the scheduled-event pump; nothing mutates the graph or clock by reaching past it. Two eras share that rule:

- **Era 1 (shipped — the offline reference-host era, present tense).** The engine API itself is the pump and the engine has a single owning caller: whoever calls `render` also owns calling `flush_scheduled` before rendering (the reference host flushes inside `wire_pending`/`wire_arranger`, `crates/host/src/lib.rs`). All event kinds may apply on the render stack except arrangement ops, which are control-side-only. As of this change, an arrangement op that nevertheless reaches the render stack is **parked** (`Engine.parked`, `crates/engine/src/render.rs`), not dropped: the block continues unaffected, and the next `flush_scheduled` applies parked ops FIFO *before* the due queue. A logged op can no longer be lost; live/replay divergence on skipped work is eliminated. The `debug_assert!` tripwire remains and fires after parking, so development builds stay loud about contract violations; a test captures it via `catch_unwind` to exercise both profiles through one path (`crates/engine/tests/parked_arrangements.rs`).
- **Era 2 (decided now, not built — binding trigger).** The first change in which a second thread controls an engine whose render is driven by an audio callback (cpal device integration or shell live transport) must land, in the same change: a fixed-capacity SPSC command ring built like `media/src/ring.rs` carrying timestamped commands to the render side; node construction and teardown moved **off** the render thread using the existing reader-park pattern (`media/src/stream.rs`) so the renderer applies pre-built values and retirees free off-thread; and the splice `Mailbox` unified onto that channel and deleted. Until then the machinery stays unbuilt per the [umbrella-first budget rule](../../proposed/architecture/2026-08-15-umbrella-first-product-direction.md) — no concurrency, no consumer, no ceremony. This supersedes the "lock-free channel in Phase 1" phrasing in the spike-B and P1-2 notes (which correctly predicted the need but tied it to the wrong milestone).

Schema rule locked alongside: pump payloads stay small values — `&'static str` keys, primitive fields; heap-owning payloads appear only in Era 2 as control-side-constructed boxes handed over once. Events remain minute-scale in size regardless.

The minimal-core note's interpreter design (Arc-swapped graphs, block-boundary crossfades) remains the Era-2 target shape; this note owns the interim contract and the migration trigger.

## Alternatives considered

- **Fail loud in both profiles** (`Result`-returning render, or panic on the render stack in release) — maximal honesty, but it churns every `render*` call site and test in the workspace, and crashing contradicts the recover-over-crash discipline every other scheduled-apply arm follows (`apply_patch`/`set_param` arms debug-assert and continue deterministically). Parking achieves the same determinism without either cost. Rejected.
- **Re-schedule instead of park** (push the op back at the block end) — keeps one queue, but mutates the op's apply frame behind the log's back, forcing replay to replicate the postponement bookkeeping to stay identical. A parked FIFO needs no frame surgery. Rejected.
- **Build the Era-2 machinery immediately** (rtrb/basedrop dependencies, Arc-swap graph) — no concurrent consumer exists; every caller is a synchronous offline host. It would violate the budget rule and ship untested against the very callback it exists for. Rejected until the trigger fires.
- **Leave the drop, upgrade the assert to release panic** — smallest diff, but the divergence trap survives for any release-mode host that forgets a flush (exactly the mistake the silent-bounce bug history predicts). Rejected.

## Consequences

- Nothing logged is ever silently dropped; the live-vs-replay divergence class on missed flushes is closed, pinned by `parked_arrangements.rs` in both profiles.
- The handoff has one owner (this note) instead of milestone-punted phrasing spread across three notes; review of render/control plumbing must cite it.
- Misuse remains quiet in release (late-apply, not error) — accepted: debug builds trip immediately, tests pin recovery, and Era 2 removes the ambiguity structurally.
- The parked vector allocates only when a violation occurs and grows with violations between flushes — bounded in practice by misuse rate; the tripwire surfaces it in the same build.
- Era 2 is prose until triggered. Risk: rediscovery under deadline pressure; mitigated by the trigger being binding and cheap to satisfy (the SPSC pattern already exists in-tree).

*Authored with GLM-5.3 Flash · ZCode, 2026-08-27.*
