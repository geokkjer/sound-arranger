# Agent Note: Clock-out in the host — a device behind a slot, and nothing across a rebuild

Status: implemented

## Problem

The engine half of clock-out ([note](2026-09-29-midi-clock-out-in-the-engine.md)) generates ticks and
transport with no device anywhere in it. Something then has to own a MIDI port, feed the transport
tap, and keep the render path clear of hardware — and, critically, something has to enforce the
recorded decision that **nothing is sent across a rebuild**, because only the host knows a rebuild is
happening.

## Decision

**`crates/media/src/midi.rs`** — `MidiOut` opens a port by case-insensitive substring of its name
(loud on zero or several matches, listing what exists) and owns it from **one writer thread**.
`EventSink::send` only converts to bytes and pushes into a bounded lock-free ring; drops are counted.
The module header states what is and is not claimed: ticks are generated at exact frames, so
**scheduling** is sample-accurate, while **wire timing** is as fast as the writer thread drains and no
jitter figure is asserted.

**The host** wires it up:

- `clock_out` is registered in `HOST_PLUGINS`; `"transport"` is provided **always** (pure
  bookkeeping), and `"midi.out"` is provided as a **slot** that always exists and is empty when no
  device was requested.
- The tap is fed from the **apply** path at the frames `Play`/`Stop` take effect: `Start` only from
  frame 0 — MIDI `Start` means "return to song start", so a resume is `Continue` — and `Stop` on stop.
  Feeding it in apply is what makes a replay re-feed it identically; silence on a replay comes from the
  empty slot, never from a special case.
- **The slot is what makes the conservative decision executable.** Around *every* rebuild render — the
  seek warm-up, undo/redo replay, and an export's clone plus its offline render — the host empties the
  slot and restores the device afterwards, with the restore on the failure path too. So a rebuild sends
  nothing, live playback drives gear, and the next `play` re-syncs. The rebuilt session shares the same
  slot `Arc`, which is why the refill is visible to its node.
- **Which device is attached is configuration, not session state**: `--midi-out <port>` or
  `DSH_MIDI_OUT`. A port name in a logged command would put a machine-specific fact in the session,
  and destinations belong to the rig binding. The snapshot carries `midi: MidiOutStatus { port,
  overflows }` next to the meters, so a shell can show whether gear is being driven.

## Alternatives considered

- **Withhold the sink from a rebuilt session** instead of emptying a slot. Rejected, and this is the
  reason the slot exists: the plugin takes its sink when the mount applies, so a rebuilt node would
  hold `None` permanently — gear would stop being driven after *any* seek.
- **Give the node a sink once and never change it.** Same defect in a different place.
- **Let an export drive gear.** Rejected: an offline render exists to produce a file, and driving
  hardware from it is a side effect nobody asked for. The export path detaches for both its rebuild and
  its render.
- **Put the port name in a logged command.** Rejected: machine-specific state in the session log, which
  is exactly what the rig's binding rule keeps out of it.
- **`midir` from crates.io.** Not possible as-is: 0.10.1 pins `alsa ^0.9`, which hard-conflicts with
  cpal 0.18's `alsa 0.11` because both declare `links = "alsa"` — cargo refuses two crates linking the
  same native library. The maintainer's master accepts `alsa >=0.9, <0.13`, so this is a **git
  dependency on unreleased 0.11**, pinned by `Cargo.lock` and commented. Revisit when 0.11 ships;
  talking to the ALSA sequencer directly through the `alsa` crate cpal already links is the cleaner
  long-term answer, at the cost of Linux-only.
- **Space the wire bytes with a timer.** Deferred: it would dress coarse wire timing as precision.
  Until it exists, the claim stays "scheduling accuracy".
- **Give `Trigger` a MIDI representation.** Rejected: it has none on this wire; it is dropped and
  *deliberately not counted* as an overflow, because nothing was due. Documented and asserted.

## Consequences

- Verified by the gates (26 suites, clippy `-D warnings`, fmt) and by tests that pin the behaviour
  rather than the intention: **a seek sends nothing while it rebuilds and resumes afterwards**; an
  export clone sends nothing; a session with no sink mounts, renders and replays byte-identically; the
  ticks land at the tempo map's frames with the transport mapped `Start`/`Continue`/`Stop`; and the
  overflow counter reaches the snapshot.
- The shell spikes are separate workspaces with their **own committed `Cargo.lock`s**, so a new
  transitive dependency has to be added to each as well. CI caught precisely that — `--locked`
  refusing to update them — while the root workspace passed, which is the class of breakage a
  root-only test run cannot see and the reason those jobs exist.
- **Nothing here has touched a real MIDI device.** The `#[ignore]`d
  `midi_out_real_port_sends_a_few_ticks` is the first thing to run when gear is available, and it is
  the only evidence that would make the wire conversion and the writer thread more than compiled code.
- `Control` maps to CC#1 on channel 0 because the event currency carries a value but neither a
  controller index nor a channel; that is a limitation of the currency, recorded rather than hidden.
- The slice sat uncommitted through a DSH service restart and survived on the filesystem alone. The
  work was not lost, but nothing protected it — the branch-per-slice discipline exists for exactly that
  window, and it was not being followed while iterating.

*Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-29.*
