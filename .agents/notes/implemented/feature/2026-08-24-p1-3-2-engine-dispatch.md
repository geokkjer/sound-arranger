# Agent Note: P1.3.2 — the closed-core plugin-message dispatch (engine)

Status: implemented

## Problem

P1.3 needs the arrangement ACID ops to be **logged commands** carrying `at_frame`, applied
and replayed — but the engine is std-only and must not grow one `Event` variant per plugin
op (the shape note's encoding decision). Until now the core could only log its own concepts
(clock/graph): mounts, patches, params. There was no way for a profile-level op (a clip
editor's `AddClip`) to be model-visible.

## Decision

The engine gains a **closed-core generic message** (`crates/engine/src/value.rs`,
`log.rs`, `render.rs`):

- `Value { Str(&'static str), U64, I64, U32, F32 }` and `OpMsg { op, fields }` — a scalar
  field list. The core never knows what an op *means*; it logs, schedules, and dispatches.
- `Event::Arrangement { op, fields, at_frame }` — the logged command. `arrange(op, fields)`
  validates (handler registered, every `F32` finite — fail-loud), logs, and schedules.
- A per-op handler registry: `register_op_handler`/`unregister_op_handler`; the plugin
  registers handlers on mount.
- `SchedEvent::Arrangement` is dispatched **on the control side only** (`flush_scheduled`);
  the render block `debug_assert!`s if it reaches one (a media handler reconciles readers —
  threads/file I/O — which must never run on the audio thread).
- `replay_from` re-schedules logged `Arrangement` events **and repopulates `self.log`**, so
  a replayed session's log still describes the audio it produces and can be continued.

**Handler contract** (documented on `OpHandler`): invoked on the control side; **atomic**
(validate then mutate — an `Err` means "nothing changed"); a **deterministic function of
the op stream** (state derives from ops alone); reconciled pool files immutable by id.

## Alternatives considered

- **One `Event` variant per op** — rejected (shape-note must-fix 9): the std-only core would
  depend on plugin vocabulary, and every new op becomes a core change. The single-envelope
  + dispatch keeps the core closed.
- **Dispatch on the render stack like mounts/patches** — rejected after the kimi slice-3a
  review: mounts are host-flushed setup; arrangements fire at edit rate, and the media
  handler cannot do file I/O/threads on the audio thread. Control-side-only (via
  `flush_scheduled`) is the split.
- **`replay_from` bounce-only (leave `self.log` empty)** — rejected (kimi must-fix 2): a
  replayed session that continues editing would save a log that doesn't match the audio.
- **`OpHandler = fn pointer`** — rejected: the media handler needs captured state (`FnMut`).

## Consequences

- The core now carries profile-level ops as opaque logged commands without knowing them;
  the same mechanism serves any future plugin (not just the clip editor).
- An op is applied at, and only at, a control-side flush; its logged `at_frame` records the
  edit's timeline position. The host renders up to each edit frame then flushes (the
  existing run_script pattern).
- `arrange` refuses a non-finite `F32` and an unregistered op, never logging either; a
  handler `Err` at apply is a `debug_assert` (release reproduces the same skip) and leaves
  engine state untouched (the handler is atomic).
- 6 engine integration tests: apply at frame, refuse-unregistered-never-logged,
  refuse-non-finite, replay determinism, replay-repopulates-log-for-continuation. 14
  workspace suites green, clippy clean.
- **Deferred**: the media-side wiring that decodes `ArrangeOp`s into `engine.arrange`
  calls + the `ClipEditor` that holds the `Timeline` and reconciles `ArrangerNode`s
  (P1.3.2b), and the references the op stream makes to immutable-by-id pool files
  (P1.3.3). This note owns only the core dispatch mechanism.
