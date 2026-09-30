# Agent Note: the iced shell records — one key, auto-named takes, a live indicator

Status: implemented

## Problem

The iced shell could play but not record, so the recorder profile — the project's current focus — was
unreachable from its second shell. The substrate was already there: `record <take_id>` opens the input
device, captures through `media::Capture` into the pool, and `record stop` finalizes the take
([note](2026-09-23-recording-into-the-host.md)). What was missing was the shell's half: a key, a name,
and something on screen that says a take is running. A take was reachable only by typing
`:record take-1` into the command line, which is not an affordance anyone would find.

Two things had to exist first. A shell may not restate what the engine declares
([rule](../architecture/2026-09-30-a-shell-derives-values-from-the-engine.md)), and the take's id must
exist **before** the recording starts — a take is declared state, so the session replays without the
device — which means the shell has to know the names the pool already holds.

## Decision

**`Action::RecordToggle` in the shared workflow; one key starts or stops a take; the shell names it
against the pool the host publishes.**

1. **The workflow owns the verb, not the shell** — `Action::RecordToggle`, on `o`
   (`r` is rewind). Both shells dispatch it, which is the point of one keymap: adding it to the iced
   shell alone would have grown a second workflow.
2. **`command_name()` is `None`** for it, beside the transport verbs. That map is the *arrange-line*
   vocabulary (`razor_split`, `trim`, …) and a test enforces that every name in it parses through
   `parse_arrange_line`; `record` is a `host v1` verb whose take id the shell must choose, so like
   `Play`/`Stop` it has no arrange-line form.
3. **The snapshot now publishes the take state it needs.** `Snapshot` gained `recording`, `last_take`
   and `pool_ids`, because the shell polls the snapshot and `HostOutcome` is a one-shot command
   response. `recording` is what makes the indicator live rather than a message that scrolls away;
   `pool_ids` is what makes the name collision-free. Both are read through the session in the same
   publish pass the meters and MIDI use, and `pool_ids` is **cached**, refreshed where the pool can
   change (`set_pool`, a finished take), because `Pool::list` reads a directory and publish runs on
   every pump tick.
4. **The iced shell auto-names** `take-N`, taking the highest `take-N.chK` the pool holds plus one, so
   a gap is never reused and non-take material does not shift the numbering. **The TUI asks**, opening
   the command line prefilled with `record `, because that is where that shell makes decisions and the
   id is a real one. The shells differ in how a name is chosen, not in what the key means.
5. **A static timeline placeholder**, not a canvas: the snapshot publishes no clips and no tracks, so
   there is nothing to draw. The placeholder reports the transport's position as a fraction of the
   furthest it has played — honest and derived — and says on screen that a real timeline needs the
   arrangement on the snapshot first.

## Alternatives considered

- **Reuse `r`.** Rejected: `r` is rewind, and moving rewind to free the mnemonic would break the
  keymaps both shells already teach. `o` is free, one key, and no modifier.
- **Name the take in the host** (a counter, or the host picking the id). Rejected: the id is declared
  state that the log carries, so the session can replay without a device; a host-chosen id would be a
  second source of truth for a name the shell already has to be able to show. The shell names it, and
  the host validates that it is free.
- **Let the iced shell keep a local counter instead of reading the pool.** Rejected: a counter resets
  with the shell and would overwrite a source the session still holds after a reload. The check
  against `pool_ids` is the difference between a name and a collision.
- **Make `set_pool` create its directory** so a fresh session can record with no shell step. Tried and
  **reverted**: two host tests build "a pool directory that does not exist" to force a refused rebuild
  (`a_refused_undo_leaves_the_history_alone`), so the refusal is load-bearing contract. The shell
  creates the directory instead — a filesystem errand that belongs to the shell anyway.
- **A real timeline canvas now.** Rejected: no arrangement on the wire. A canvas would be drawn against
  data the shell would have to invent, in the same host the split will later re-arrange.
- **Have the TUI auto-name too.** Rejected: the TUI has a command line and uses it for every value a
  person should choose (`rename_track` prefills the same way). Auto-naming there would make the second
  shell's behaviour depend on a pool listing it does not read.

## Consequences

- **The recorder is reachable in both shells.** `o` records; the iced shell shows a live
  `● REC take-N — frames, ch, dropped` line and reports the finished take with its pool sources.
- **Verified against real hardware, not a fixture**: `cargo run -- --record-check` drives the whole
  path through the real host and a Scarlett 2i2 — `take-2` captured **57,856 frames, 2 ch, 0 dropped**,
  left `take-2.ch0`/`.ch1` in the pool, and correctly chose `take-2` because `take-1` was there. The
  live counter climbed while it ran (21,504 → 33,792 → 45,568 → 57,344). Two tests pin the naming rule
  and the indicator's three states.
- **The capture is pumped while the transport runs**, which the check exposed: a take started with the
  transport stopped reports `0 frames` until play. That is the existing design (the pump drives the
  capture), not a defect introduced here, but it is now written down — a shell that says "recording"
  while the transport is stopped is telling the truth about the device and not about the take.
- **`--record-check` is the instrument for this verb**, as `--sweep` is for a fader drag: a GUI
  interaction cannot be scripted from outside, so the shell drives itself. It needs an input device, so
  it is not a CI gate — the owner runs it.
- **The toggle's start-vs-stop decision has a test, device-gated rather than mocked**
  (`record_toggle::the_toggle_starts_then_stops_a_take`). It reads the host rather than the repaint and
  asserts the started id equals what `next_take_id` derived — pinning `take-1` would pass once on a
  clean pool and fail on the next run, which is a test that only works the first time. With no input
  device (CI) it verifies the refusal path instead and says which branch it took, so a skipped
  assertion cannot read as a pass.
- **Two bugs came out of building it, both from the same shape** — acting on state that was not the
  host's: the pool was set *before* `host.load`, which replaces the session wholesale and threw it
  away, and `toggle_record` decided start-vs-stop from the last repaint, so a key press could act on a
  stale take state. Both are fixed by ordering and by reading the host at the moment of the decision.

Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-30.
