# Agent Note: recording binds a declared source — `record <take> source=<name>`

Status: implemented

## Problem

The rig declaration was **state with nothing behind it**: `source add synth kind=alsa
match=hw:USB channels=8 clock=master` was logged, replayed and saved, but `record` always opened
the machine's **default input device**
([the rig declaration is state](../architecture/2026-09-27-the-rig-declaration-is-state.md) named
the binding as "a later slice"). For an external software partner — VCV Rack through the Scarlett's
sink monitor, a `pw-loopback` — that meant changing the system's default source for the whole
session, and the take could not say what it had captured. The
[VCV contract](../../proposed/architecture/2026-10-08-recording-vcv-rack-as-a-partner-contract.md)
is the decision this implements; the studio rig's sketch (`source add vcv --kind jack --match
"VCV Rack:out_1"`) assumed a JACK port capture that does not exist on this box.

Measured before writing anything (this machine, 2026-10-08, PipeWire 1.6.9 with `pipewire-alsa`):

- **A PipeWire node is not a cpal device.** cpal/ALSA enumerates `Default Audio Device`,
  `PipeWire Sound Server`, `PulseAudio Sound Server`, `JACK Audio Connection Kit` and hardware
  names (`Scarlett 2i2 4th Gen, USB Audio`, `Felucca, USB Audio`). The sink monitor
  (`alsa_output…HiFi__Line1__sink.monitor`) appears in none of them.
- **`PIPEWIRE_NODE` and `PIPEWIRE_PROPS = { node.target = … }` were ignored**: `arecord -D
  pipewire` with the default source set to a silent microphone stayed silent (peak 0.0013).
- **`PULSE_SOURCE` worked**: with the default source still the microphone, `arecord -D pulse`
  carrying `PULSE_SOURCE=<monitor>` captured a tone played to the sink at peak 0.41 (−7.7 dBFS),
  while the no-hint control on the same sink captured 0.44.

So per-source capture is real today through the PulseAudio-protocol PCM, which is what PipeWire
also speaks.

## Decision

**`record <take_id> [source=<name>]` — the name is the rig's, and the binding never guesses.**

- **Omitted** means the default input, exactly as before: every existing script and session
  records unchanged.
- **`kind=alsa`** binds a **cpal device name** by case-insensitive substring (`Scarlett 2i2 4th
  Gen, USB Audio`, `Default Audio Device`). A rule, never an index.
- **`kind=pulse`** binds a **PulseAudio/PipeWire source name** — a sink `.monitor`, a
  `pw-loopback` — opened through the `pulse` PCM with `PULSE_SOURCE` set for the open only. This
  is the route for capturing an external program by name.
- **`jack` and `osc` stay declared seams with no binding**: naming one is refused
  (`kind 'jack' … has no binding yet (kinds with a binding: alsa, pulse)`), never silently
  captured from somewhere else.
- **An unknown name is refused** with the declared names listed — the recorder cannot fall back to
  the default input for a source the session asked for by name.
- **Provenance is the declaration**: the take line gains a trailing optional `source=<name>`
  (`take jam 96000 0 2 120 source=vcv`), and `TakeReport`/`RecordingStatus` carry it, so a shell
  and a reloaded session say what was recorded, not just what the machine's default happened to be.
- **`PULSE_SOURCE` is process-global and `unsafe` to set in edition 2024**, so the window is
  serialised behind `media::devices::DEVICE_ENV_LOCK` and the previous value is restored before
  the open returns; the stream keeps the source it connected to.
- **The shells arm the source.** `A` cycles `default input → the rig's declared sources → default`
  ([`Action::CycleRecordSource`](../../../../crates/workflow/src/lib.rs), so both shells share one
  key, one help row and one meaning). The rig is published as names in the snapshot
  (`Snapshot::sources`), `o` records the armed name, and the footer names it. The TUI prefills its
  record prompt with the armed source and its next take id, so the line that runs is the line on
  screen rather than the source being appended behind the user's back. A rig with nothing declared
  arms `default` and says so.

## Verified

- `cargo test --workspace` — including `record_resolves_a_declared_source_or_refuses_loudly`
  (undeclared name, unbound kind, typo-with-declared-names) and
  `a_take_declaration_carries_its_source` (parse, replays, formats back, and the pre-binding line
  is unchanged).
- `media` unit tests pin the matcher rule (substring, case-insensitive, empty is not a wildcard)
  and the no-match refusal.
- Both shells: `the_source_key_cycles_the_declared_rig` (iced) and
  `the_source_key_arms_the_declared_rig_for_the_next_take` (tui) declare a rig through the host,
  cycle it, and assert the armed name and the prefill — iced 21, tui 58 tests, green.
- **On hardware** (ignored by default): `record_binds_a_declared_pulse_source` declares the default
  sink's monitor as `kind=pulse`, records through `record bound source=mon`, and asserts the take
  named its source, captured frames, and produced `bound.ch{k}` pool sources — run and passing
  2026-10-08.

## Alternatives considered

- **Bind cpal device names only (ignore `pulse`).** Rejected: a sink monitor or a loopback is not a
  cpal device, so the one case the contract exists for — capturing Rack — would remain unroutable.
- **Overload `kind=alsa` for both mechanisms.** Rejected: a device name and a server source are
  different rules with different failure modes; picking one silently is how a session captures the
  wrong thing.
- **A native PipeWire/`pipewire-rs` client now.** Deferred, not rejected: it is the clean answer to
  per-node selection without a process-global hint, but it is a new dependency and a larger slice
  while `PULSE_SOURCE` demonstrably works.
- **Set `PIPEWIRE_NODE`/`PIPEWIRE_PROPS` instead of `PULSE_SOURCE`.** Rejected on measurement: both
  were ignored on this PipeWire version.
- **Make the source permanent session state and bind at load.** Rejected: binding opens a device,
  and replay must never touch hardware — the same rule the take declaration follows.
- **Keep the source choice on the command line only, with no shell gesture.** Rejected: the command
  line stays (scripts need it), but a record key that cannot pick a declared source quietly records
  the default input for anyone who does not know the modifier exists. `A` arms the choice in both
  shells; the footer names what is armed.
- **Two simultaneous sources per session.** Out of scope: one take records one source, which is
  what `HostSession`'s single active capture already models.

## Consequences

- A session says what it recorded: `session.txt` carries `source add …` and the take's
  `source=<name>`, so the binding is reproducible **by name**. The *routing* behind it (a loopback
  or monitor existing, WirePlumber restoring it after a reboot) is still external, and the contract
  says so.
- `SOURCE_KINDS` gained `pulse` — a closed registry, additive. An older host refuses the line at
  parse (the documented growth rule), rather than ignoring a kind it cannot bind.
- The take line gained one optional trailing modifier; old lines parse unchanged.
- `PULSE_SOURCE` names one source per open. A second simultaneous source would need the native
  client — recorded here rather than discovered later.
- The monitor route captures the **whole sink**, so the arranger's own playback lands in the take
  if it plays there; a `pw-loopback` is the isolation route the contract describes.
- **One bug fell out of the shell work:** the TUI's help overlay clamped `help_scroll` to the number
  of keymap *entries* while ratatui scrolls *wrapped lines*, so on a short terminal the last
  bindings (from `m` down) could not be read. The clamp is now three wrapped lines per entry and the
  test checks **reachability across every reachable scroll stop** instead of "the token appears
  somewhere on screen" (which had been passing on unrelated text).

*Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-10-08. The owner's session chose the
slice and approved the plan; the measurement that picked `PULSE_SOURCE` is in this note.*
