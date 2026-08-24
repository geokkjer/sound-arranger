# Agent Note: P1.3.4 — the reference host speaks the clip-arrangement commands

Status: implemented

## Problem

The host (`crates/host`) had only the ad-hoc `Play`/`Splice` media path. The clip editor's
ACID ops (P1.3.0/2b) are logged commands that reconstruct a `Timeline` value, but the host
had no way to issue them as commands or point itself at the media pool — so a reference host
couldn't speak the arrangement at all. `Play`/`Splice` remain for the recorder/player path;
P1.3.4 adds the arrangement command surface.

## Decision

`HostCommand::Arrange { op, at_frame }` (applies an `ArrangeOp` via the `ClipEditor`, which
dry-runs, logs, and flushes) and `HostCommand::Pool { dir }` (calls `set_pool`, building the
stem-id → path resolver). `HostSession` holds the `ClipEditor` lazily + the pool resolver;
`ensure_editor` registers the op handlers; `arrangement()` exposes a read-only snapshot of
the constructed value.

The host now builds the arrangement value as logged commands and replays it
byte-identically: the value is a pure reconstruction of the op stream (the log-visibility
carve-out), verified by the `arrange_commands_build_the_logged_value_and_replay_identically`
test (live value == replay value, with track/clip/fade fields asserted).

**The live audio wiring of the arranger nodes into the mixer is deferred and documented**:
the mixer (the bus) is applied at graph index 0 during the first `ClipEditor::apply` flush,
*before* the arranger nodes exist, so `arranger → mixer` is a backward cord under the graph's
forward-order rule. Fixing it requires the shared-state `ArrangerNode` reconcile plus the
mixer applied last (the dynamic-edit architecture), not a one-shot snapshot. A one-shot
wiring attempt was removed rather than shipped as a silent/vacuous path (a naive test
passed a **silent** bounce).

## Alternatives considered

- **One-shot wiring at Bounce (arranger nodes built from the final timeline)** — rejected:
  the cord is backward (mixer index 0 before the arranger nodes), and a naive test passed a
  silent bounce (vacuous). Removed.
- **`wired_tracks` + recompute on `arrange_dirty`** — rejected as the one-shot path (same
  backward-order + stale-node problems); the correct fix is the shared-state reconcile.
- **Defer the mixer mount globally** — rejected: breaks the existing `Play`/`Splice` tests
  (patches to the mixer validate against a scheduled/mounted mixer).
- **`parse_script` text for `arrange`/`pool`** — deferred (the text-format parser expansion is
  a follow-up; the command surface is programmatic for now).

## Consequences

- The host can build an arrangement as logged commands and reconstruct it byte-identically;
  `Arrange` without `set_pool` is refused (fail-loud, never logged).
- The arrangement *value* is now host-visible (`arrangement()`), so a shell can read the
  timeline even before the audio wiring lands.
- 2 host tests (arrangement value + replay identity; refuse-without-pool) + the existing 6
  reference-host tests pass. 17 workspace suites green, clippy clean.
- **Deferred**: the arranger→mixer audio wiring (shared-state `ArrangerNode` reconcile +
  mixer-last ordering); `parse_script` text for `arrange`/`pool`; the drain/EOF phase for
  tailed effects (a separate proposed note — the arranger's clips are finite, so a bounce
  truncation concern is for future reverb/delay/codec nodes, not the clip editor per se).
