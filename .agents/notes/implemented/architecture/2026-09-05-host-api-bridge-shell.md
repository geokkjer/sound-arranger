# Agent Note: Host-API bridge — the Tauri shell can drive the engine

Status: implemented

## Problem

The configured goal's next step was to wire the scaffolded Tauri/Vue shell to the
Host API (`docs/design/ui-plan.md`'s "Next", the UI-as-plugin note). The shell
(`crates/shell/src-tauri`) had only a `main.rs` that opened a window and served
the Vue frontend; it did not depend on `crates/host`, so the engine was
unreachable from the frontend. Exactly as the note predicted: "wiring the
`HostCommand`/events/values contract comes next."

## Decision

The shell owns a live `host::HostSession` behind Tauri commands. Shipped:

- **The shell depends on the reference host** (`crates/shell/src-tauri/Cargo.toml`
  adds `host = { path = "../../host" }`).
- **`run_host_script` command** (`crates/shell/src-tauri/src/lib.rs`): takes the
  versioned text-format script, `host::parse_script` + `host::run_script` it, and
  returns a `ScriptOutcome` (summary, underruns, deferred, event_count,
  media_commands, arrangement_tracks, whether a Bounce was written). A refused
  bad script or op surfaces as an `Err` the UI can show, never a panic.
- **The bridge is unit-testable outside a Tauri runtime**: the command's body is a
  plain `exec_host_script(&str)` fn; the `#[tauri::command]` wrapper is a thin
  `run_host_script(String)` that calls it. `run()` builds the Tauri app and
  registers `run_host_script` via `generate_handler!`.
- **Lib/bin naming**: the lib is `sound_arranger_shell_lib` (`[lib] name`), so
  `main.rs` calls `sound_arranger_shell_lib::run()` without the bin/lib
  same-name self-reference ambiguity.

- **The arrangement value is serializable** (`crates/media/src/timeline.rs`):
  `Timeline`/`Track`/`Clip`/`Edge` derive `Serialize`/`Deserialize` (media gained
  the `serde` dep), so the value can cross the IPC boundary as JSON for the
  canvas.
- **`ScriptOutcome` carries the full arrangement** (the shell's bridge): the
  `run_host_script` command returns `arrangement: Option<media::Timeline>` in
  addition to the diagnostics, so the frontend can draw the timeline in one
  round-trip.

The versioned text format is the shell's wire schema — the same contract the CLI
smoke binary drives, so a script that bounces byte-identically on the CLI behaves
the same here. This is the transport layer: it makes the engine reachable and
returns the arrangement value, so the `ui-plugin`s (timeline canvas, source pool,
mixer) can be built on top (the next step), not those views themselves.

## Consequences

- The frontend can invoke `run_host_script` and get the session diagnostics it
  needs to show engine state; the underlying `exec_host_script` is covered by two
  `cargo test` cases (a tone+bounce script runs with 0 underruns and one logged
  engine event; a script missing the `host v1` header is refused cleanly).
- `cargo test --workspace` stays green; the shell crate has no clippy warnings.
- A live **incremental session API** is now exposed on the host crate:
  `HostSession::new()` is public, `HostSession::execute(&HostCommand)` applies one
  command onto a persistent session (advancing to its frame, validating the mixer
  mount — the exact per-command logic `run_script` uses, shared via
  `process`), and `arrangement()` returns the serializable value. A host test
  (`incremental_session_applies_edits_and_reports_arrangement`) builds and bounces
  an arrangement edit-by-edit and verifies a refused op changes nothing. This is
  the live path the frontend will use; the canvas/pool/mixer `ui-plugin`s are the
  next sub-step.

## Alternatives considered

- **Surface `&mut Engine`/`HostSession` directly to the frontend:** couples the
  UI to engine internals and breaks the UI-as-plugin boundary (the Host API is the
  seam). Rejected.
- **Call `host::run_script` from `main.rs` directly, no lib:** keeps the shell a
  pure bin, but makes the command untestable and hard to grow into an incremental
  API. Rejected in favour of a lib with a testable `exec_host_script`.
- **Reuse `media::bounce` for the bridge instead of the host text format:** the
  shell should speak the Host API (the documented seam), not reach into
  `media` directly. Rejected.

*Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-05.*
