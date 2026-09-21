# Agent Note: integration is external programs and devices — not in-process sidecars

Status: proposed

## Problem

The sidecar/plugin roadmap has been accumulating scope that the product does not need. As written it
promises **VST3/CLAP builds** of our capabilities, a **CDP / offline-process sidecar** binary, and (via
the composition-seams and umbrella-first notes) an eventual plugin *host* with CLAP/LV2/VST support.
Each of those is a product in its own right: a plugin-format build chain, a distribution and
licensing surface (CDP is LGPL, `vst3-sys` is GPLv3), an editor for other DAWs, and a host that has
to track three plugin SDKs.

Meanwhile the capabilities those artifacts were meant to provide already exist as **better
standalone programs** — TidalCycles for generative performance, VCV Rack for modular synthesis, CDP8
/ sox / ffmpeg for offline processing, and the owner's own hardware (the M-AVE FM-1 and the EP-133)
for sound sources — all of them Linux/JACK-native and all of them already good at their job. The
arranger's actual role in that world is narrow and clear: **provide the clicks and the timing, and
collect the audio as recordings.**

The prior-art study also says where this project's moat is: not DSP breadth, but the
**non-destructive clip model over a replayable log** ([study](../../../../research/architecture/2026-09-21-tui-audio-prior-art.md) §5,
item 9). Every hour spent on plugin plumbing is an hour not spent there.

## Proposal

1. **We do not ship in-process sidecars or plugin-format builds as a product goal.** No VST3/CLAP
   builds, no CDP sidecar binary, no plugin host. `plugins/` stops being a promise and becomes a
   pointer to this note.
2. **Integration is at the process/device boundary.** A partner is something we can *start or
   address*, *sync*, and *record*: its audio arrives in the media pool as a normal take, and its
   identity is a property of the source, not of our link-edited binary.
3. **Clock and transport out is a first-class, in-engine capability** — the half we *do* own:
   - **MIDI clock / start / stop / SPP** over a MIDI output (ALSA/JACK) for USB-MIDI and DIN gear;
   - **audio-encoded sync**: a click/pulse train — or audio-encoded MIDI — written to a dedicated
     output channel, for gear that takes analog sync. This is not exotic: it is exactly what
     [Expert Sleepers' USAMO](https://www.expert-sleepers.co.uk/usamo.html) (a plugin generates MIDI
     as *audio*, the box reconstructs sample-accurate MIDI from a jack input) and
     [Bastl's KLIK](https://www.synthtopia.com/content/2017/01/20/bastl-introduces-klik-audio-click-to-modular-clock-converter/)
     (audio click → modular clock) do as hardware. **Because the signal is audio, and the engine
     renders audio, we can generate it ourselves** — no sidecar, and sample-synced to the same clock
     as the arrangement by construction.
4. **Offline processing means invoking external programs.** Render (or point at) a source, run the
   program (CDP8, sox, ffmpeg, `tui-wave`, …), import the result as a **new pool source**, and log
   the command that asked for it. The `OfflineProcess` *seam* stays as the description of that
   workflow; it is no longer a plugin format we implement.
5. **Hardware is treated exactly like software partners**: capture its output into the pool, send it
   clock/MIDI, and keep its device-specific facts where device facts live — the studio project
   (`~/Projects/music/music-composition-theory/studio/instruments/<name>/`), *not* here. This note
   owns the software contract (how we sync, capture and log); the gear matrix (which device takes
   MIDI clock vs analog pulse vs DIN sync, which interface emits a usable pulse, what cables) is
   gear research and belongs there, referenced not copied.
6. **The internal plugin model is untouched.** "Everything is a plugin" still describes the *engine*
   (`Plugin` trait, the graph, the log); this decision is about the **product boundary**, which
   becomes process/device-level rather than binary-level.

## Alternatives considered

- **Build the VST3/CLAP artifacts and a CLAP host (the previous plan).** Rejected as product scope:
  it makes us maintain plugin scaffolding, per-format builds and a licensing surface to deliver
  capabilities that standalone programs already deliver better. It can return later as an *export*
  capability ("render this arrangement into a plugin-friendly artifact") if a real need appears —
  that is a feature, not the architecture.
- **Keep the CDP sidecar binary.** Rejected: CDP is an external program with its own packaging and an
  LGPL distribution story; wrapping and shipping it buys nothing over invoking it and importing the
  result, while adding a binary we must build and version.
- **Go the other way: build in-engine DSP breadth (more synths and effects as core plugins).**
  Rejected as a *goal* — the engine keeps the minimal core plus the few building blocks the arranger
  itself needs (e.g. the fundsp-backed voice), and everything else is somebody else's program. This
  is the scope-creep brake the decision is meant to be.
- **Be the host that hosts everything (CLAP + LV2 + VST + JACK graph).** Rejected: that is a
  different product (a modular host), and the unclaimed ground the study found is the **terminal
  arranger**, not another plugin host.
- **Do nothing (leave the sidecar roadmap as written).** Rejected: an unowned promise in `plugins/`
  and RESEARCH §6 is exactly the kind of scope that quietly becomes work.

## Acceptance criteria

1. **A written integration contract per partner** — TidalCycles, VCV Rack, and the hardware sources:
   how the session starts/addressing it, what sync it takes, how its audio arrives (JACK ports,
   device capture, or a file), and how a take lands in the pool with its provenance.
2. **Clock out works end-to-end and is measured**: the engine emits MIDI clock (ALSA/JACK MIDI
   output) *and* an audio-encoded sync signal on a chosen output channel, and a device follows it —
   with **drift measured over at least ten minutes** and published, not assumed. (Computer MIDI
   jitter is the reason USAMO exists; if we generate the signal, jitter is our number to own.)
3. **Record-while-synced**: a session records a partner's output as a take that stays aligned with
   the arrangement (the existing capture path and `DriftCompensator` do the work), and the take is
   replayable from the log like any other source.
4. **Offline round-trip**: an external process runs on a rendered temp file and its output is
   imported as a new pool source, driven by logged commands.
5. **The docs stop promising the old thing**: `plugins/`, RESEARCH §6 and §16.7, and the notes that
   assumed sidecar artifacts are updated or superseded in the same change that lands this decision.

## Risks

- **JACK/ALSA session plumbing is real work**: device ownership, port routing, period/sample-rate
  alignment between us and a partner, and the difference between capturing a device and capturing
  another *client*. This replaces plugin plumbing with session plumbing — cheaper, but not free.
- **Jitter and drift are the classic failure**, and now they are ours: MIDI clock from a
  general-purpose machine is notoriously uneven (again: USAMO is a product whose whole reason is
  this). The audio-encoded path is the escape hatch, because it inherits the audio clock.
- **Sync classes are not interchangeable**: MIDI clock vs analog pulse vs DIN sync (24/48) vs MTC vs
  Ableton Link — per-device, and a matrix is needed. That matrix is gear research and lives in the
  studio project.
- **The partner's state is not in our log.** We log *what we asked for* (start, tempo, sync), not what
  the program did; a take is therefore the durable artifact, and reproducibility of a
  generator-driven piece rests on the recording, not on replaying the generator. This is an honest
  limit, not a defect — and it is the same epistemic position as recording an external instrument.
- **A process boundary is harder to test than a function call.** Each integration needs an
  `#[ignore]`d integration test and a documented manual procedure; the alternative (mocking a
  TidalCycles session) would test nothing.

## Attribution

Authored with DeepSeek-v4-flash · DeepSeek Harness, 2026-09-21. The direction is the human's —
replace sidecars with integration ("we provide clicks/timing and collect audio as recordings"), with
TidalCycles and VCV Rack as the first partners; the sync-hardware facts were verified against the
vendors' own pages (USAMO) and contemporary coverage (KLIK).
