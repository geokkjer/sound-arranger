# kimi review — P1.3.2 engine arrangement dispatch (slice 3a, 2026-08-24)

Session: `session_8ccaeabf-60f6-4497-9205-57b845789e43`

kimi reviewed the closed-core plugin-message dispatch (`Value`/`OpMsg`,
`Event::Arrangement`, `register_op_handler`/`arrange`, `SchedEvent::Arrangement`,
`replay_from`) before commit. Its verdict, accurate:

> The closed-core boundary is genuinely maintained and the single-envelope design is
> the right call, but the slice has one architectural problem that will bite the moment
> the media handler is real — dispatch runs on the render stack — plus a replay-contract
> hole. **Not merge-ready as-is.**

## What's right (confirmed by kimi)

- No plugin vocabulary in the core; `Event::Arrangement` carries an opaque op + scalar
  fields. The core can log/frame-schedule/replay ops it doesn't understand — the goal.
- `arrange` validates-then-logs-then-schedules; a refused op is never logged.
- Same-frame ordering is deterministic (`Scheduler` inserts after equal frames; replay
  schedules in log order). Verified, not assumed.
- The handler cannot reenter the engine; `FnMut` (captured state) over a fn pointer is right.

## Must-fix (both integrated)

1. **The handler ran on the render stack, and the intended media handler cannot survive
   that.** `render_block` → `apply_event` dispatched the handler inline mid-block; a media
   handler reconciling readers (threads, file I/O) would block/allocate on the audio thread
   and violate the no-alloc invariant. **Resolved:** `Arrangement` now dispatches only on
   the control side (`flush_scheduled`); the render block sees an `Arrangement` event and
   `debug_assert!`s (the host must flush before rendering to that frame). `arrange`
   documents the flush requirement. The op stays logged; replay reproduces the same skip.
2. **`replay_from` left `self.log` empty — a replayed session could not continue.** After
   replay, the engine's log described nothing, so a "load, edit, save" session produced a
   log that didn't match the audio. **Resolved:** `replay_from` repopulates `self.log` with
   each replayed event (pushed after a successful arm, so a validation failure still never
   logs a refused event, and continuation appends to a coherent log).

## Should-fix (integrated)

- **`arrange` finiteness validation** — now refuses a non-finite `F32` field (matching
  `set_tempo`/`set_param`); NaN would break `Event::PartialEq` and the determinism tests.
- **`OpHandler` contract made explicit**: invoked on the control side, must be **atomic**
  (validate then mutate; `Err` means "nothing changed") and a **deterministic function of
  the op stream** (state derives from ops alone); reconciled pool files immutable by id.
- **Mount→arrange same-burst trap documented** on `arrange` (handlers register at apply, so
  flush between mount and first arrange).

## Worth-considering (left as flags)

- `Value` is sufficient for the listed ops, but `OpMsg`'s lopsided u64/f32 getters hide
  type errors; symmetric typed getters (or dropping `OpMsg`) is a follow-up. Duplicate
  field names aren't rejected. `&'static str` interning leaks unbounded ids (fine at spike
  scale; needs a table on log serialization). `Engine` is already `!Send` (`Disposer`,
  `Box<dyn Plugin>`); adding `+ Send` bounds across the whole struct is a future decision.
- Byte-identity is a **media-side proof obligation**: the handler must be a pure function
  of the op stream, and files referenced by id immutable — the media side should carry the
  replay test that rebuilds the timeline from ops only.

The full critique text is the reviewer's response in this session.
