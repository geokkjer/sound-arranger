# Agent Note: a remounted mixer is rewired, and the bus takes the arranger wiring with it

Status: implemented

## Problem

The reference host wires the arranger by **reconcile-on-dirty** (the
[reconcile note](../feature/2026-08-25-arranger-reconcile-on-dirty.md)): an `Arrange` op
mutates the value, `arrange_dirty` goes true, and the next render re-derives the graph from
that value. `unmount mixer` was the one thing that took the **bus** away without touching
any of it. The arm cleared the host's *player*-side mixer state — `pending_cords`, the
mailbox, the counters, `mixer_channels` (the hardening
[wave](../feature/2026-08-24-hardening-wave-guard-park-rate-render.md)) — and left
`wired_tracks`, `arranger_underruns` and `arrange_dirty` exactly as they were
(`crates/host/src/lib.rs`, the `Unmount` arm).

Nothing else could have retired those nodes. The mixer's own disposer removes **only the
mixer node** (`crates/engine/src/plugins/mixer.rs`: `remove_node` + `ctx.remove`), and
`Graph::remove_node` keeps every cord that does not touch the node it removes — so each
`ArrangerNode` stayed mounted with its arranger→mixer cord dropped, still rendering every
block, still reading its pool file, still counted by `underruns()`. And because
`arrange_dirty` was still `false`, `wire_arranger` returned on its first line, so a mixer
mounted later got **no inputs at all**:

```text
mount mixer channels=2 @0
pool <dir>
arrange add_track t0 @0
arrange add_clip t0 c0 s1 0 4800 0 0 0 1.0 @0
bounce 4800 /tmp/a.wav        # renders, wires the arranger, arrange_dirty := false
unmount mixer
bounce 10 /tmp/x.wav          # the render that applies the scheduled unmount
mount mixer channels=2
bounce 4800 /tmp/b.wav        # ← silence, no error
```

The session reports a full arrangement, `underruns() == 0`, and emits nothing. Reported as
MAJOR #4 by the [Space Bunny
review](../../../../research/architecture/2026-09-29-space-bunny-review-4-host.md), which
also found no test covering it: the one test that unmounts the mixer
(`params_fold_from_the_log`) has no arrangement and no re-mount.

## Decision

**The bus going away retires the wiring the same way a reconcile does, and a session that
owes a bus is recorded as owing one.**

- **`retire_arranger_wiring` is one function** (`HostSession::retire_arranger_wiring`): drain
  `wired_tracks`, `graph.remove_node` every id, clear `arranger_underruns`.
  `wire_arranger` calls it in its commit phase — which is where the counter clear moved to,
  so a *refused* reconcile now leaves `underruns()` describing the nodes still mounted
  rather than nothing — and the `Unmount { plugin: "mixer" }` arm calls it too. One rule, one
  place: the leak could not have been fixed by a second copy of the teardown that drifts.
- **`unmount mixer` marks the arrangement dirty**, so a re-mounted bus reconciles on its next
  render exactly like an edit does.
- **`arranger_awaiting_bus` records the debt, and it is what the dirty flag alone cannot
  say.** Two sessions can be "dirty with no mixer mounted", and they are not the same
  question. One is *a window of the session's own timeline*: `mount mixer` / `unmount mixer
  @96 000` / `mount mixer @192 000` is a session the live path accepts and records, so
  `process` renders up to a command's placement frame straight through the bus-less window
  — that is a legitimate silence, and the debt is settled by the next bus. The other is *an
  arrangement with no bus anywhere*, which is a session that cannot render and has been
  refused with `arrange requires the mixer to be mounted` since `render` returned a
  `Result`. `wire_arranger`'s no-bus branch answers the first with silence (unwired, still
  dirty) and keeps the refusal for the second. The flag is set only when the wiring was
  actually non-empty when the bus left, and cleared by the reconcile that settles it.

## Alternatives considered

- **Mark the arrangement dirty on `unmount mixer` and let the missing bus refuse** (the
  review's suggested fix, verbatim). Rejected because it breaks a shape the platform
  supports: with no bus, the `Mount mixer @192 000` in `build_mixer`'s
  `a_session_that_re_mounts_a_plugin_still_exports_and_seeks` fails — `process` renders the
  bus-less window on the way to the placement frame, `wire_arranger` refuses there, and the
  re-mount, the export and every seek go with it. Silence through the window is what that
  session already rendered; the defect was what came *after* it.
- **Retire the nodes but leave `arrange_dirty` clear** — the leak dies, the re-mounted bus
  stays unwired, and the reported symptom (a silent bounce) survives. The re-mount is the
  half a user actually sees.
- **Always tolerate a missing bus** (drop the refusal, no flag). Rejected: it silences a
  real mistake — an arrangement built with no mixer at all — that the `Result`-returning
  `render` has named since the hardening wave, and it makes the two bus-less cases
  indistinguishable rather than answering both.
- **Retire the orphans in `wire_arranger` instead, by noticing the mixer is gone** — one
  place, but the flag still has to exist to tell a window from a session that never had a
  bus, and the nodes would survive until the next render rather than going when the bus
  does.
- **Ask the engine whether a mixer mount is queued** (`Engine::scheduled`) and treat a
  queued bus as a window. Rejected: it needs a new engine accessor for a host-local
  question, and it makes the *placement* decide the semantics — a session that unmounts the
  mixer and never mounts it again would be treated differently from the same session
  written with the re-mount, although the two render identically until the re-mount lands.
- **A separate `RetireArranger` command** (a user gesture for "forget the wiring").
  Rejected: nothing needs to be spelled, and a command that only re-derives a value the
  host already holds is a second way to be wrong.

## Consequences

- **One regression test, beside its module's neighbours** (`crates/host/tests/arranger_commands.rs`,
  with the other reconcile tests):
  `a_remounted_mixer_is_rewired_and_leaves_no_orphan_nodes` bounces a wired arrangement,
  unmounts the mixer, renders the bus-less window, mounts the mixer again and bounces
  again. It asserts the graph is the mixer plus one arranger, that the window renders
  silence and leaves **no** node behind, and that the re-mounted bus is wired again and
  carries the arrangement. Verified to fail on the unfixed code on **both** halves — the
  orphan count (1 vs 0) and the silent re-mounted bounce — so neither is load-bearing by
  accident.
- **A session that unmounts the mixer and never mounts it again** stays dirty for the rest
  of its life and renders the silence of a bus-less session, which is the audio it rendered
  before this change. Each render costs one `flush_scheduled` and a `node_of` lookup; no
  readers are warmed, which is the point (the old leak warmed them forever).
- **The render path is untouched.** All of this is control side, in the `Unmount` arm and in
  `wire_arranger`, which the render path already called. No allocation, no lock, no new
  panic; a no-bus window is an `Ok`, not an `Err`.
- **The [reconcile note](../feature/2026-08-25-arranger-reconcile-on-dirty.md) is updated in
  place** with the bus-leaves case, so the one description of the wiring contract covers it.
- **`unmount mixer` is the only trigger.** Every path that unmounts it runs the host's
  `Unmount` arm (the live `execute`, a `rebuild`, a `rebuild_prefix` and `load_session` all
  re-issue state through `apply`), so no path reaches the engine's disposer for the mixer
  without the host having retired the wiring first.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
