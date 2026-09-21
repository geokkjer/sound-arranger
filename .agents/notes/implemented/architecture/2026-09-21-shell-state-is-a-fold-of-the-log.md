# Agent Note: a shell's state is a fold of the log

Status: implemented

## Problem

The TUI shell's mixer had its own copy of the fader positions, starting at unity. That copy drifted
from the engine the moment anything else changed the session — loading a script, replaying the log,
or an undo — and it forced every shell to re-implement "what is the value of this parameter?" with
no authority behind the answer. A shell that mirrors engine state is a second source of truth, and
this project's whole discipline says there is one: *the log is the command list* (minimal-core note
§3, "model-visible means logged").

The concrete failure: `--script` a session whose script sets `ch0.gain 0.3`, and the console shows
1.00. The engine is at 0.3; the shell is showing its own default. Nothing is broken, but nothing is
true either.

## Decision

**The Host API publishes `params`, and a shell derives its parameter state from it.** The value is a
**fold of the session log** — computed on demand, never stored:

- `Mount { params }` contributes a plugin's initial values; `SetParam` overwrites (last write wins);
  `ScheduleUnmount` removes that plugin's entries, and a later mount re-adds them.
- Order is first-appearance, so a replay produces an identical `Vec` and a shell can diff two reads.
- `HostSession::params()` is the fold; `HostOutcome::params` carries it across the seam, so any shell
  reads it in the same round trip as the arrangement (`crates/host/src/lib.rs`, `live.rs`).

The TUI shell adopts it on every load and refresh (`App::apply_params`) — after `--wave`, after
`--script`, after a clip edit, and after undo/redo. Its faders are now a **view of the log**, not a
mirror of it: if the log says `ch1.mute 1`, the strip shows muted, whoever asked for it.

Two properties fall out, and they are the point:

1. **A log replay restores shell state for free.** Undo rebuilds the session by replaying the log
   (`HostSession::undo` → `replay_to`), so the parameters come back exactly as logged. Undoing a clip
   split therefore leaves a fader ride *intact* — correct, because a fader ride is not an arrangement
   edit — and the console proves it: the strip returns to the value the replay put there.
2. **A shell cannot invent state.** There is no path by which the console shows a value the log does
   not contain. That is what makes the shell interchangeable: every shell reads the same fold.

## Consequences

- The TUI's mixer gap is closed: a reload, a script load, an edit and an undo all leave the console
  agreeing with the engine. Verified by test (`faders_are_read_from_the_log_and_survive_an_undo`):
  a script sets `ch0.gain 0.3` / `master.gain 0.6` / `ch1.mute 1`, the strips show exactly that, a
  split takes the clip count to 2, and `u` brings it back to 1 with the faders untouched.
- The fold is O(log events) per read, on the actor thread, only when a shell asks (a load or a
  refresh) — not per frame. A live 60 Hz parameter readout would want the fold cached against a log
  length; that is a follow-up, not a need today.
- `params` sits beside the other values the contract already carried (`providers`, the graph, the
  arrangement, the pool listing), so the Host API's shape is unchanged: commands, events, values.
- Other shells should adopt the same discipline (the Tauri shell reads `HostOutcome` already; the
  iced spike does not yet show parameters). The rule to keep: **no shell stores what the log owns.**

## Alternatives considered

- **Put `params` in the published `Snapshot`.** It would remove the round trip, but the snapshot is
  cloned by every shell on every frame, and a parameter vector is a heap allocation per tick for data
  that changes rarely. Rejected for now; the on-demand `HostOutcome` read matches how often shells
  actually need it (load, edit, undo). A cached fold keyed on `log.len()` is the cheap upgrade if a
  live parameter display ever wants it.
- **Keep the shell's own copy, refreshed on its own edits.** That is what the spike did, and it is
  exactly the bug: any change the shell did not make (a script load, a replay, another actor) is
  invisible. Rejected.
- **Have the engine expose a typed parameter registry per plugin** (so a shell enumerates params
  rather than parsing names). A real improvement in ergonomics for a richer control surface, and it
  is the direction the host registry (`HOST_PARAMS`) already hints at — but it is a bigger contract
  change than this need justifies, and the string names it would replace are already validated by the
  parser. Deferred, not rejected.
- **Reconstruct parameters in the shell by replaying the *text* of a script.** Duplicates the
  engine's semantics (mount defaults, unmount) in the wrong place, and cannot see commands the shell
  did not send. Rejected.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-21.
