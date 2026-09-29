# Agent Note: shell live bridge — a persistent session with transport commands

Status: implemented

## Problem

The Tauri bridge was **stateless**: `run_host_script` called `host::run_script`, which built a fresh
`HostSession` per call and returned the diagnostics. That is fine for "load a script, draw the
result," but it cannot host a transport — the session (and therefore the playhead and playing state)
did not outlive the call. Wiring the live transport needs three things the bridge did not have:
`Send + Sync` Tauri state for the session, a way to load a script into the *live* session without
double-mounting on a re-run, and first-class transport commands plus a poll.

## Decision

The bridge (`crates/shell/src-tauri`) is now a thin adapter over [`host::live::HostHandle`]:

- **Tauri state is the `HostHandle`.** `HostHandle` is `Send + Sync` (it holds a `Mutex<Sender<…>>`,
  an `Arc<Mutex<Snapshot>>`, and the join handle), so `.manage(HostHandle::spawn())` compiles — unlike
  `State<Mutex<HostSession>>` (`HostSession` is `!Send`; the live-runtime note owns that).
- **`run_host_script` = reset-and-apply into the live session** (`load_script`): parse the text →
  `HostHandle::load` (the actor builds a *fresh* session, adopts it only once it built whole, and
  returns the `HostOutcome`) → map the outcome + the current snapshot to the serializable
  `ScriptOutcome`. Reset-and-apply keeps re-running a script **idempotent** (no double-mount),
  preserving the old stateless semantics while the session stays resident for transport. A
  **refused** script changes nothing: the `Err` is returned and the session that was already there
  keeps playing ([the note that fixed it](../bug-fix/2026-09-29-a-refused-live-load-keeps-the-session.md)),
  the same rule `load_session` follows.
- **Transport is first-class, not script text**: `transport_play` / `transport_stop` /
  `transport_seek(frame)` execute the corresponding `HostCommand`; `transport_state` polls the shared
  snapshot and returns `TransportState` (position + meters + the pump's `last_error`). The live,
  high-frequency controls do not belong in the versioned text schema.
- **`ScriptOutcome` grew** `position`, `channel_count`, and `arrangement_error` (the arrangement
  snapshot error, surfaced instead of a silent empty timeline); `mixer_meters` now comes from the
  live snapshot. `channel_count` drives the mixer strip count.
- The command bodies are plain fns over `&HostHandle` (`load_script`), so they stay unit-testable
  without a Tauri runtime.

Tests: a tone+bounce script loads with clean diagnostics and a channel count; a script missing the
`host v` header is refused; load → play → poll (the playhead advances) → stop over the live handle.

## Alternatives considered

- **Keep the stateless one-shot bridge and add a separate live session for transport** — two sessions
  over the same contract, which can diverge ("which one is playing?"). Rejected: one live session.
- **`run_host_script` appends to the live session (no reset)** — re-running a script would re-mount
  the mixer and fail ("already mounted"), and the previous state would leak into the new run.
  Rejected: reset-and-apply is idempotent.
- **Reset-and-apply, and install the empty session when the script is refused** — a rejected
  alternative, kept here because the failure is what the idempotence was for: the shell gets its
  error *and* loses the arrangement, the pool binding, the undo history and the transport. Rejected:
  building the candidate and adopting it only on `Ok` is idempotent in both directions, and matches
  `load_session`.
- **Expose transport only as script text (`transport play` lines)** — the frontend would drive
  playback by re-running a script, and a poll would need a script run per tick. Rejected: the text
  schema is for loading a session; transport is a live control surface.
- **Poll meters/position via a request round-trip to the actor** — would serialize every poll behind
  the actor's render work. Rejected: the pump already publishes a shared snapshot; the poll reads it.

## Consequences

- The frontend is wired to the live bridge: `Surface.vue` polls `transport_state` at ~25 Hz (and
  stops itself if the bridge is unreachable, e.g. a plain `pnpm dev` browser), drives
  `transport_play`/`transport_stop`, and shows the polled BPM + timecode; `TimelineCanvas.vue` fits
  the arrangement to the canvas, draws the live playhead at the polled frame, and a click seeks
  (`transport_seek`). The shared `bridgeState` (`src/bridge.ts`) grew
  `position`/`channelCount`/`lastError`.
- The text contract still drives the same session (`transport play|stop|seek` parse); the CLI and the
  shell share the command vocabulary.
- Device audio output is still absent (the pump renders into the void); the transport ticks and the
  meters move, but nothing is heard yet — the deferred device-output step.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-09.

Authored with Space Bunny · OpenCode, 2026-09-29.
