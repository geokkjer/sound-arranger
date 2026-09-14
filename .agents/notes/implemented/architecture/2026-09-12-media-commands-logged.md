# Agent Note: Media commands are logged — pool/play/splice ride the session log

Status: implemented

## Problem

The engine's log carried the engine commands (mount/patch/set_param/set_tempo/unmount)
and the clip editor's arrangement ops, but the **media commands did not**:
`pool`/`play`/`splice`/`bounce` mutated the host's graph directly
(`HostSession::apply`) and were recorded only in a host-side counter. So
`Engine::replay_from` could not reproduce a session that played or spliced a
clip, seek/undo rebuilt from a *separate* host `history`, and the README's
"determinism is split-brain" stood: media determinism rested on a parallel
command seam and matched discipline, not one log. The Phase-1 profile note had
recorded the intent ("the recorder/clip-editor absorb it into logged events");
this note is that step.

## Decision

Media commands are **profile-level ops carried by the one session log** — the
same closed-core `Event::Arrangement` carrier the clip editor uses
(`host::media_ops`):

- **`MediaPool` / `MediaPlay` / `MediaSplice`** are encoded to `op` + `fields`
  (interned paths as `Value::Str`, frames as `Value::U64`, channel/count as
  `Value::U32`), logged via `Engine::arrange_logged` (**logged, never scheduled** —
  the live path applies once and cannot be parked at a call boundary), and decoded
  by handlers the host registers on a fresh engine.
- The handlers mutate a shared **`MediaSession` value** (pool dir, player intent,
  splice intents) and do **no I/O and no graph work**, so `replay_from` rebuilds
  the media value on a fresh engine — the clip editor's value/reconcile split.
- **`MediaBounce`** is a **record**, not state: frames rendered plus the drain
  outcome (`drained` tail frames, `capped`). It is logged after the bounce and
  never re-executed — a bounce writes a file and is not session state. The output
  **path is deliberately not recorded**: it is an action target, and recording it
  would make two identical sessions bouncing to different files log differently.
- `Play`/`Splice`/`Pool` are now **state** (`is_state`): seek/undo/redo replay
  them from the host history, and the log carries them for engine-level replay.
- **Stage-0 replay correctness** (from the independent review):
  - `Engine::replay_from` **fails loud** when a logged op has no handler
    registered, instead of scheduling an op that is silently dropped at apply
    (release: `debug_assert` only) and yielding a session with no media state and
    no error.
  - `replay_to` **skips history state that takes effect after the seek target**,
    so a backward seek is no longer a no-op forced forward by a later command's
    `at_frame` (a pre-existing bug the new media state commands made common).

`Record` stays unlogged (the device path is unwired and refused before logging),
and the transport ops are deliberately **not** logged: they change no rendered
audio, and a logged seek would make replay O(target).

## Alternatives considered

- **Keep the media commands host-side and unlogged** — the status quo; media
  state is invisible to `replay_from` and determinism stays split. Rejected.
- **A new `Event::Media` variant** — puts file/playback semantics into the
  "model-free closed core", the one thing the architecture forbids; it is
  `Arrangement` under a new name. Rejected.
- **Give `OpHandler` an engine-side context (`&mut DisposerCtx`) so a handler can
  add nodes itself** — hands plugin code `&mut` graph/scheduler internals,
  reopens the reviewed `OpHandler` contract, and lets a handler mutate topology at
  a render boundary (the parked-mixer hazard class). Rejected.
- **Move the file open/warm into a `wire_media` reconcile now** — the reconcile
  needs a reader anchor (`off0 = from_frame - start`) or a deferred `Play` starts
  late and at the clip head. Kept the open+warm in `apply` (control side, exactly
  once, before the first block that pops the ring); a reconcile, when it lands,
  should only insert/wire. Deferred, recorded below.
- **Log the bounce output path** — makes the log depend on the export target, so
  two identical sessions bouncing to different files log differently. Rejected.
- **Log the transport (`TransportPlay`/`Stop`/`Seek`)** — no rendered-audio
  effect, and a logged seek makes replay O(target). Rejected.

## Consequences

- The session log now carries the media commands, so `replay_from` reconstructs
  the media *value*; the graph and the reader threads stay host-side, exactly as
  the timeline/arranger split (`replay_from` rebuilds the Timeline, the host wires
  the `ArrangerNode`s).
- `pool`/`play`/`splice` are seek/undo/redo state, so a session that played a clip
  survives a seek.
- Tests: the media ops appear in the log and the count matches the media counter;
  a refused media command is never logged; `replay_from` rejects a handler-less
  op; a backward seek before a later state change lands at the target.
- **Not yet shipped (recorded from the review):**
  - **Handlers are frame-blind.** `OpHandler` receives fields, not the event's
    `at_frame`, and `SchedEvent::Arrangement` carries no frame. Harmless while the
    graph work is eager host-side, but the follow-on reconcile must pass the frame
    (or carry a distinct command-frame field; note clip ops already use a field
    named `at_frame` for clip *placement*, so keep field names scoped per op).
  - **The mailbox is imperative, not a value.** Splices are pushed into a
    `VecDeque` drained at block boundaries; a value-level reconcile must instead
    keep splices as absolute-frame state armed once.
  - **Two replay authorities.** The host `history` Vec and the engine log are both
    hand-maintained; `history` should be derived from the log (a decode) or
    retired.
  - **Paths are interned absolute strings.** The `Interner` leaks every unique
    string, and an absolute path bakes the working directory into the log; the
    pool's source-*id* form is the right key for pool-resolved clips.
  - **The drain is still not a logged event** (the drain note owns that), so a
    session that bounces and continues still has an unlogged clock advance.
  - **The supersession question.** The P1.3 clip-editor note decided the host's
    `Play`/`Splice` are *superseded* by the arrangement commands. Logging them is
    justified by the recorder's audition path (monitoring a raw clip); if that
    path is retired in favour of an arrangement op, the media ops should retire
    with it.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-12.
