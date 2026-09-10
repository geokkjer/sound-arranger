# Agent Note: live runtime — the host actor thread and the real-time render pump

Status: implemented

## Problem

A live transport needs a `HostSession` that **persists across commands** and advances in time. The
existing bridge was stateless (a fresh session per `run_host_script`), and the session cannot simply
be shared: `HostSession` is `!Send` (the engine holds `Box<dyn FnOnce>`/`Box<dyn FnMut>` disposers
and op handlers plus a `Box<dyn Any>` context), so `tauri::State<Mutex<HostSession>>` is a *compile
error* (the Tauri review's F2). Moving the session into `thread::spawn` is equally impossible — the
closure would capture a `!Send` value.

## Decision

Add `host::live` (host crate only — the review's "live runtime, still no Tauri"), an **actor**:

- **The session is constructed *inside* the host thread.** `HostHandle::spawn()` spawns a closure
  that captures only `Receiver<Request>` and `Arc<Mutex<Snapshot>>` (both `Send`); `HostSession::new()`
  runs on the thread. Nothing `!Send` crosses the boundary — no engine change, no `+ Send` bounds.
- **`HostHandle` is `Send + Sync`**: `Mutex<Sender<Request>>` + `Arc<Mutex<Snapshot>>` + a
  `Mutex<Option<JoinHandle>>`. `execute(&HostCommand)` sends a request and blocks on a reply channel,
  so a refused command surfaces as `Err` and the session stays usable.
- **The pump**: the actor loop waits up to `TICK` (4 ms) for a command, then — while playing — renders
  the frames the wall clock says are due, anchored at `(Instant, frame)` on play and re-anchored on
  any transport command (so play/stop/seek never jump). Bursts are capped at `MAX_CHUNK` frames.
- **Value access is a shared snapshot, not a query**: the pump publishes `Snapshot` (frame, seconds,
  beat, bpm, playing, per-channel meters, master) into `Arc<Mutex<Snapshot>>`; `snapshot()` clones it.
  `HostCommand` is `Send`, so no engine type crosses the thread boundary.
- **A render error is never silent**: if the pump's `render` fails (a wiring failure), it records the
  error in `Snapshot::last_error` and stops the transport, rather than leaving a moving, silent
  playhead.
- **Host-crate only and headless-tested**: `cargo test -p host` covers play→advance→stop (the clock
  holds still when stopped) and command/refusal survival. The Tauri bridge is a thin adapter.

## Alternatives considered

- **`tauri::State<Mutex<HostSession>>`** — the default Tauri pattern. Rejected: a compile error, not a
  runtime hazard (`HostSession: !Send`).
- **Add `+ Send` to the engine's `Disposer`/`OpHandler` and `Box<dyn Any + Send + Sync>` in `Context`**
  — a small, honest core change the review notes would also unlock parallel offline bounces.
  Rejected *for now*: not required for the first shell, and the actor keeps the change surface zero.
  Kept as a possible later cleanup.
- **Move the session into `thread::spawn`** — also impossible (`!Send` capture). The session must be
  constructed on the thread; that is exactly what the actor does.
- **A frontend-driven timer issuing `render_block` commands** — workable, but it puts the clock in the
  UI and couples realtime pacing to IPC jitter. Rejected: the host thread owns time.
- **Publish meters via a per-poll query round-trip** — would serialize every meter poll behind the
  actor's render work. Rejected: the pump publishes them into the shared snapshot instead.

## Consequences

- A shell holds a `HostHandle`, sends `HostCommand`s, and polls `snapshot()` for the playhead + meters;
  the actor thread is the only sanctioned threading model for the session (and the physical guarantee
  that the UI never touches the render path).
- The pump **renders into the void** today (no device output): the same pump will fill an `Spsc`/device
  ring in the device-output step (C), which is the milestone the user deferred.
- `session.render()` allocates its output `Vec` per tick — acceptable on this control-side actor, and
  the device-output step should switch to a preallocated `render_into` scratch buffer.
- Device sample-rate negotiation (the review's F6) is untouched and still the device-output step's job.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-09.
