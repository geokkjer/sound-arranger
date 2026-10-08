# Agent Note: recording VCV Rack as a partner — the v1 contract

Status: proposed

## Problem

[Integration is external programs](2026-09-21-external-programs-not-sidecars.md)
makes a partner something we can *start or address*, *sync*, and *record*, and its first
acceptance criterion is **a written contract per partner**: how the session starts or addresses
it, what sync it takes, how its audio arrives, and how a take lands in the pool with provenance.
VCV Rack has none — the [route study](../../../../research/architecture/2026-08-19-softsynth-integration-routes.md)
is analysis, and the studio rig's sketch (`source add vcv --kind jack --match "VCV Rack:out_1"`,
[the live rig](../../../../../music/music-composition-theory/studio/the-live-rig.md)) assumes a
JACK port capture we do not have.

Two things on this machine sharpen the contract (owner's session, 2026-10-08):

- **Rack Free 2.6.6 works through PulseAudio/PipeWire, not JACK.** Its log shows the one stream it
  ever opened: `Opening RtAudio PulseAudio device 130: Scarlett 2i2 4th Gen (2 in, 2 out, 48000
  sample rate, 256 block size)`. Rack probes ALSA, PulseAudio and JACK drivers at startup; the
  freeze the owner hit is the **ALSA device entry** (`(1-2 in, 1-2 out)`), which tries to take the
  Scarlett's hardware while PipeWire owns it. The JACK entry needs `pw-jack` (there is no `jackd`;
  PipeWire provides JACK). So **v1 must not depend on JACK.**
- **Capture is a PipeWire source, and binding to one is the real gap.** `HostSession::record`
  opens the machine's **default input device** — there is no per-source choice. The rig
  declaration exists and is state ([the rig declaration is state](../../implemented/architecture/2026-09-27-the-rig-declaration-is-state.md)),
  but binding a declared `matcher` to a real device is explicitly the later slice; until it lands,
  "record Rack" means changing the machine's default source for the whole session.

## Proposal

**v1 — Rack on PulseAudio/PipeWire, captured by name from a declared source.**

1. **Addressing.** The owner starts Rack; the session *addresses* it, never spawns or supervises
   it. The source is declared with the existing vocabulary, e.g.
   `source add vcv kind=alsa match="Scarlett 2i2 4th Gen" channels=2 clock=free`
   (a PipeWire source name, matched by pattern like the studio's `pw-link` scripts — never a device
   index). `kind=jack` stays a declared-but-unbound name in v1.
2. **The capture tap.** Two routes, both PipeWire sources:
   - **Sink monitor (v1 default).** Rack → PA → the Scarlett sink; capture
     `alsa_output.usb-Focusrite_Scarlett_2i2_4th_Gen_*.HiFi__Line1__sink.monitor`. Zero new nodes,
     matches how Rack already runs. Cost: the tap is the **whole sink**, so our own playback lands
     in the take if it plays there too.
   - **`pw-loopback` (isolation).** A dedicated virtual source that carries only Rack. One extra
     persisted node, and it is what makes the studio rig's "two VCV instances" sketch playable.
   The take's provenance names which tap it was; the choice is per session, not global.
3. **Binding (the missing software).** `record <take> source=<name>` resolves the declared
   `matcher` to a capture device and sizes the take from it. Concretely on this box, cpal/ALSA
   enumerates the `pipewire` and `pulse` PCMs, and a *specific* PipeWire source is selected by
   opening that PCM with a node hint (`PIPEWIRE_NODE` / `PULSE_SOURCE`) — a process-level hint, so
   the slice must set and restore it around the stream open, and a **native PipeWire client** is
   the clean long-term replacement. Until the slice lands, the owner can record today by making
   the monitor the default source; the session then cannot name what it recorded from.
4. **Sync.** v1 sends **MIDI clock out** (`clock_out`) to Rack's MIDI-CV, so Rack follows our tempo
   and start/stop. No JACK transport and no shared position beyond MIDI clock/SPP. A free-running
   take is acceptable: per the external-programs decision the **recording is the durable artifact**,
   not the generator's state.
5. **The take and its provenance.** The take lands as `{take}.ch0/ch1` in the pool and is declared
   as state (`take <id> <frames> <dropped> <channels> <at_frame>`), with the source name added when
   binding lands. Rack's patch path and plugin versions are recorded as *references*, never as a
   promise of audio reproduction. The take's counters keep their separate meanings — monitor drops
   are monitor drops, not take loss (the fix landed with the iced-spike finding of 2026-10-08).
6. **Not in v1.** JACK-direct port capture; JACK transport; spawning/supervising Rack; the
   hardware cable loopback; Rack Pro as a CLAP plugin (the
   [composer note](2026-10-08-generative-composer-and-clap-host.md)'s optional later integration,
   which hosts Rack inside us rather than capturing it).

## Alternatives considered

- **JACK-direct (`kind=jack`, `match "VCV Rack:out_1"`).** Deferred, not rejected: it is the studio
  rig's sketch and the right shape once transport following matters, but it needs a PipeWire/JACK
  client we do not have (cpal is ALSA; no JACK crate in the lockfile), and Rack's JACK entry does
  not currently run for the owner.
- **Hardware loopback (Scarlett output → cable → input).** Works with today's code and is the
  fallback, but the 2i2 has no internal loopback and it costs a cable and an input pair.
- **`snd-aloop` kernel loopback.** The same idea as `pw-loopback` with a kernel module to load and
  a device to manage; PipeWire's loopback is already installed here.
- **Make the monitor the permanent default source.** Cheapest of all and recordable today, but it is
  a machine-wide setting that outlives the session, and every take loses the name of what it
  recorded. Rejected as the contract; kept as the interim instruction.
- **Spawn and supervise Rack from the session.** Out of scope: a partner is something we start *or*
  address, and addressing does not make the platform a process manager.

## Acceptance criteria

- A session declares `source add vcv …`, and `record` captures **that** source rather than the
  machine's default input, with the source name in the take's declaration.
- With Rack playing into the Scarlett sink, a take lands in the pool at the session rate with both
  channels present, and replays without Rack, PipeWire or the device — the existing take rules.
- A missing or unmatched source is a loud refusal; the recorder never silently falls back to the
  default input.
- The tap is stated in provenance (monitor or loopback), and the WirePlumber caveat is written
  down: a manually made link or loopback may need re-creating after a reboot or device change, so
  re-binding is a routing step, not a session edit.

## Risks

- **The monitor route records the whole sink**, including our own playback. Mitigation: the loopback
  route for isolation, and a provenance line that says which tap was used.
- **The interim default-source instruction is unlogged.** Until binding lands a take may name no
  source; the session still replays (the WAV is the artifact), but the provenance is incomplete.
  Mitigation: keep the binding slice small and land it with the contract.
- **Device-clock mismatch** between Rack's PA stream and our capture is real and only bounded, not
  removed; `Capture`'s drift compensation converts to session frames, measured per device as the
  capture note requires.
- **Per-source selection via a process-level env hint** is the crude part of the binding slice and
  it is per-open, not per-stream: a second simultaneous source would need the native client.
  Mitigation: one source per take in v1, and record the limitation rather than hide it.

*Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-10-08. Evidence is the owner's machine:
Rack Free 2.6.6 via PulseAudio on a Scarlett 2i2 4th Gen, PipeWire 1.6.9 with `pipewire-alsa` and
`pipewire-jack`. Gear/backend findings belong in the studio project, referenced here, not copied.*
