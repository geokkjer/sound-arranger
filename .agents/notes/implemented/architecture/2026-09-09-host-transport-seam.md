# Agent Note: host transport seam — TransportPlay/Stop/Seek + position() (seek-via-rebuild)

Status: implemented

## Problem

The Host API had **no transport**. `HostCommand` could mount, patch, set params, arrange, and
bounce, but there was no play/stop/seek and no way to read the playhead; the clock only advanced
when `render(frames)` was called (`HostSession` renders offline). This is the Tauri review's F1 —
"play/pause/stop/seek/edit-while-playing *is* the product," and every panel (playhead, meters,
mixer) is downstream of a ticking transport. A shell cannot draw a playhead or drive playback
through the seam without it, and the sanctioned workaround (`engine_ref()`) is exactly what the
UI-as-plugin note forbids.

## Decision

Add a host-owned transport tier to `crates/host` (no engine change):

- **Commands**: `HostCommand::TransportPlay`, `TransportStop`, `TransportSeek { frame }`. They are
  "now" operations (no `at_frame`); `at_frame` remains catch-up-then-apply, not future scheduling.
- **`HostSession::position() -> Position`** (`frame`, `seconds`, `beat`, `bpm`, `playing`). Reading
  it never renders. `bpm`/`beat` are derived through the tempo map (the log's time-basis rule).
  `is_playing()` exposes the transport flag alone. `Position` is a plain struct — `host` stays
  `serde`-free; the shell serializes.
- **Seek = rebuild + render-to-target** (`seek_to`), the review's minimal honest v1. `HostSession`
  records the **state** commands it applied (`is_state()`: mount/patch/set_param/set_tempo/unmount/
  arrange/pool); a seek replays them onto a fresh session through the same `process` path
  `run_script` uses, then `render_to(frame)`. Correct by construction over the deterministic log;
  O(target), because the core clock only advances by rendering. Actions (transport ops, `Bounce`,
  and the one-shot player path `Play`/`Splice`/`Record`) are **not** state and are not replayed.
- **`render_to(frame)`** renders forward in 1-second chunks so a long seek cannot trip the
  single-bounce byte budget. The transport's playing state survives a seek; a refused replay leaves
  the original session untouched.
- **Text grammar**: `transport play` · `transport stop` · `transport seek <frame>`.
- **Offline semantics**: the reference host records transport state only — `Bounce` renders
  regardless, so every existing script is unchanged. The live runtime (the next unit) reads
  `is_playing()`/`position()` and pumps blocks while playing.

Tests: play/stop toggles the flag; forward seek renders to the target and reports derived musical
time; backward seek rebuilds and preserves the arrangement value; the text grammar parses.

## Alternatives considered

- **Add `Clock::seek(frame)` + re-wire the arranger readers** — cheaper than a rebuild and gives
  correct *arrangement* audio, but plugin internal state (oscillator phase, delay lines, filter
  memory) is discontinuous across a jump, so it is not "the state you would have heard at F."
  Rejected for v1 in favour of the review's rebuild (correct by construction); kept as a possible
  optimization once a cheaper exact seek is needed.
- **A transport position decoupled from the render clock** — would desync the arranger's
  `source_frame_at` math (which is absolute-frame based) from the audio. Rejected: position *is*
  rendered-so-far in this architecture.
- **Future-scheduled transport (`transport play @frame`)** — `at_frame` is catch-up, and the engine
  exposes no true future scheduling on its public API. Rejected for v1; transport ops are "now."
- **Record every command in the history** — replaying a `Bounce`/`Play` on a seek would re-run a
  side effect (write a file, start a player) and `Play` refuses a second player. Rejected: only
  state commands are recorded.

## Consequences

- A shell can now read the playhead (`position()`) and drive transport over the contract; the
  offline host's behaviour is unchanged (transport is state, `Bounce` is the renderer).
- Seek is correct-by-construction but O(target) — acceptable for v1 (short arrangements); a
  wall-clock-anchored pump and cheap seek are separate, later concerns.
- `Position` and the transport commands are the seam the live actor/runtime (next unit) and the
  Tauri bridge build on; no engine change was needed.
- Pre-existing clippy warnings untouched (a `new_without_default` on `HostSession`, a
  `chunks_exact_mut` in the engine mixer) — unrelated to this change, left for a separate cleanup.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-09.
