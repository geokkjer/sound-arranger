# Agent Note: P1.3.4 — the reference host speaks the clip-arrangement commands

Status: implemented

## Problem

The host (`crates/host`) had only the ad-hoc `Play`/`Splice` media path. The clip editor's
ACID ops (P1.3.0/2b) are logged commands that reconstruct a `Timeline` value, but the host
had no way to issue them as commands or point itself at the media pool. `Play`/`Splice`
remain for the recorder/player path; P1.3.4 adds the arrangement command surface and wires
the arranger into the mixer.

## Decision

`HostCommand::Arrange { op, at_frame }` (applies an `ArrangeOp` via the `ClipEditor`) and
`HostCommand::Pool { dir }` (calls `set_pool`, building the stem-id → path resolver).
`HostSession` holds the `ClipEditor` lazily + the pool resolver; `ensure_editor` registers
the op handlers; `arrangement()` exposes a read-only snapshot of the value.

**Value-level ops are eager + log-only** (engine `arrange_logged` + `ClipEditor::apply`
applies to the value directly): an arrangement op updates the live `Timeline` immediately and
is logged (for replay) without touching the scheduler. This is what makes the **mixer-last
wiring** possible — because the ops never flush the mixer early, the mixer (scheduled by its
Mount) applies *after* the arranger source nodes, so `arranger → mixer` is a forward cord
under the graph's topological rule. `wire_arranger` builds one `ArrangerNode` per track from
the final value (adds them first), flushes (mixer last), then connects to `mixer ch{i}`. The
mixer's `set_out` overrides the transient out_node claim of the first source.

The reference host is **build-then-bounce**: apply all ops, then `Bounce` wires + renders.
A byte-identical, **non-silent** arrangement bounce verifies the whole path (previously a
naive test passed a *vacuous* silent bounce).

## Alternatives considered

- **One-shot wiring at Bounce without the eager change** — rejected: the mixer was flushed at
  index 0 during the first arrange op, before the arranger nodes existed, so the cord was
  backward and the bounce silent. The eager+log-only change is what makes the mixer apply
  last.
- **`wired_tracks` caching for re-wiring** — kept for the build-then-bounce path, but **an
  edit *after* a bounce cannot be re-wired this way**: once the mixer is applied, a new
  arranger source is added after it (backward again). That needs the shared-state
  `ArrangerNode` reconcile + the mixer re-applied last (the dynamic-edit architecture) —
  deferred (see Consequences).
- **Defer the mixer mount globally** — rejected: breaks the existing `Play`/`Splice` tests
  (patches to the mixer validate against a scheduled/mounted mixer).
- **`parse_script` text for `arrange`/`pool`** — deferred (the text-format parser expansion is
  a follow-up; the command surface is programmatic for now).

## Consequences

- The host builds an arrangement as logged commands, and `Bounce` renders it through the
  mixer (audio, not silence) — byte-identically on replay. A refused `Arrange` (no `set_pool`)
  is fail-loud and never logged. `arrangement()` exposes the value.
- The arranger is wired by **reconcile-on-dirty** (2026-08-25): on each render after an edit,
  `wire_arranger` builds every new node (validating before mutating), tears down the old ones,
  and re-inserts one `ArrangerNode` per current track *before* the mixer (a new
  `Graph::insert_before` keeps `arranger → mixer` a forward cord), mapped to `ch{track_index}`.
  Readers are positioned at the **current transport frame**, so a mid-play edit continues the clip
  rather than restarting it. This fixes the `RemoveTrack` **ghost** (a removed track's node is
  retired) and makes edits to an already-wired track reach audio — see the [reconcile note](.agents/notes/implemented/feature/2026-08-25-arranger-reconcile-on-dirty.md).
- 10 host arrangement tests (in `arranger_commands.rs`): build value + replay identity,
  refuse-without-pool, byte-identical non-silent arrangement bounce, removed-track-does-not-ghost,
  edit-continues-the-clip (ramp source, asserts the correct region plays),
  edit-then-rewire-replays-byte-identically, bounce-over-budget, mixer-channel refusals, and the
  text-format parse + byte-identical-bounce pair. 18 workspace suites green, clippy clean.
- **Deferred**: the shared-state `ArrangerNode` + mixer-last re-application (the *dynamic-edit*
  architecture — a reconcile that reuses readers rather than rebuilding them, needed for live
  edit-while-playing where re-warming on every edit is too heavy); `render()` panics on a bad
  script via `.expect` (make it `Result`); `parse_script` text for `arrange`/`pool`; the
  drain/EOF phase for tailed effects (a separate proposed note — the arranger's clips are finite).
