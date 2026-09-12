# Agent Note: Hardware-instrument profile — drive an external synth over MIDI, capture its USB audio

Status: proposed

## Problem

The clip-arranger records long live jams, and a large share of the sounds worth arranging
come from **external hardware synths**. The M-VAVE FM-1 is the first one in the rig with
**native USB audio *and* USB MIDI** — so the host can both *drive* it (notes / CC / program
change / SysEx) and *capture* its output (stereo 24-bit/44.1 kHz over USB) with no analog mixer
in between. The gear characterization is the studio project's job and lives in the
[FM-1 research note](../../../../../music/music-composition-theory/studio/instruments/m-vave-fm-1/research-note.md)
(summarised in [RESEARCH.md](../../../../RESEARCH.md)).

What is missing is not interop plumbing for its own sake. It is a **product profile**: send our
notes and parameters to the hardware, record what it plays, and have both live in the same
session so a take is editable, recallable, and reproducible. Today the engine declares
`MidiSink`/`EventSink` (and `OscSink`) but has no implementation, and there is no profile that
turns "a synth on the desk" into a composition workflow.

**Division of labour with the sibling notes:**

- [Input/jam layer](2026-09-01-input-jam-layer-device-registry-io.md) — the *seams*
  (`Device`/`Rig` registry, normalized `AudioIn/Out` + `MidiIn/Out`, `DeviceControl`). An
  **interoperability** concern, explicitly not the product.
- [Software engine + hardware interface](2026-09-03-software-engine-hardware-interface-and-scaling.md)
  — hardware as a **control surface** (input *to* us) and as a DSP-scaling question.
- **This note** — hardware as a **sound source we drive and record** (output *from* us, audio
  back). A **product profile**, built on the seams above; it adds no engine variant.

## Proposal

An **external-instrument profile** ("hardware-in-the-loop") assembled from existing seams, with
the FM-1 as the worked example:

1. **MIDI-out as a logged sink.** Implement the already-declared `EventSink`/`MidiSink` seam with
   a `midir`-backed provider. Note/CC/PC events come from the same generators that drive software
   voices, are **logged at their absolute frame**, and are flushed to the device by the control
   path (never on the render stack). Consequence: the printed part is *in the log* — replayable,
   inspectable, and editable by editing the log, not by re-recording.
2. **Patch programming (`DeviceControl` + SysEx).** A small DX7-compatible SysEx codec (32-voice
   bulk dump: 4,104 bytes, Yamaha checksum) plus the FM-1's documented effects CC block. This is
   the librarian/editor half: load a bank, send a voice to the edit buffer, select a program by
   PC. Implemented from the **documented DX7 format** and the published FM-1 CC table — not from
   any third-party code (see prior art).
3. **Capture as a take (`Capture` → media pool).** Record the synth's USB audio into the pool
   exactly like any other input (`media::devices` / `media::capture`): live 256-sample peak
   pyramid, crash-recoverable float WAV, device-clock drift compensation. The captured take is
   the arrangement source; the MIDI part is the intent.
4. **The session log coordinates.** One log carries the performance events; the captured audio is
   the recorded artifact. **Determinism boundary, stated honestly:** the *log* is deterministic
   and replays; the *hardware's output* is **not** reproducible sample-for-sample (firmware,
   converter, knob state). So we **record the audio and re-render nothing** — replay reproduces
   the intent, the take is ground truth.

**Prior art — reference only, no license.** The
[fm1-dx7-patch-importer](https://github.com/benny-sparra/fm1-dx7-patch-importer) ("M-VAVE FM1
Editor & Librarian") is a browser-based DX7 voice editor and librarian for this exact synth, over
Web MIDI/SysEx (React, WebMidi.js). It is **unlicensed** — no `LICENSE` file, which defaults to
*all rights reserved* — and it bundles Yamaha-copyright DX7 factory banks. Disposition: **read it
as protocol/interop evidence only; copy no code, no bundled bank data, and no assets.** The DX7
32-voice bulk SysEx format is a documented standard we can implement independently, and the
repo's `docs/fm1-research.md` is useful evidence for FM-1-specific behaviour. Its feature set is a
good capability checklist for this profile (bank librarian, live voice editing, program-change
slot selection, effects-chain editing, SysEx monitor).

**Scope guard.** This is a *profile*, not a new core concern. MIDI-out stays behind `MidiSink`,
patch programming stays behind a `DeviceControl`, capture stays in `media`. If a capability does
not serve "drive the box and record it into the arrangement," it is out.

## Alternatives considered

- **Capture-only (record the synth; no MIDI driving).** The `Capture` path already works, so this
  is the cheapest increment. Rejected as *the profile*: it discards recall and editability — the
  part would live in the hardware, not the session, and a take could not be re-aimed. Retained as
  the fallback for hardware with no usable MIDI.
- **Re-synthesize the instrument in software instead** (native soft-synth / Cardinal — see the
  [native soft-synth note](2026-08-30-native-soft-synth-building-blocks.md)). Rejected as a
  *replacement*: the specific instrument (its converter, firmware voice, physical controls) is the
  sound source; an emulation is a different sound and a different project. Kept as a complement.
- **Use the importer's code or data.** Rejected: **no license** (default all rights reserved) and
  bundled Yamaha-copyright banks. Reference only; implement the documented format ourselves.
- **Route the synth through the analog mixer instead of USB audio.** Already possible (the
  Notepad-12FX path) and sometimes preferable. Rejected as the *default* for the FM-1: native USB
  audio is fewer conversions and a per-device channel, and it is precisely why this synth is the
  worked example.
- **Fold this into the input/jam-layer note.** Rejected: that note is bounded to interoperability
  paper cuts and explicitly subordinate to the product. Drive-and-capture is a product workflow
  that *uses* the jam layer's seams; it deserves its own profile entry.
- **Model full hardware state in the log (deterministic recall by re-sending patches).** Partly
  useful (PC and voice are loggable), but FX, sequencer, and analog state are not observable.
  Rejected as a determinism *claim*; keep the boundary at "log the intent, record the audio."

## Acceptance criteria

- A `MidiSink` implementation sends note/CC/PC from logged events to a real device with
  sample-accurate intent frames; a session with MIDI-out + capture replays its **log**
  byte-identically (audio sourced from the same take).
- A DX7-format SysEx codec (bulk-dump parse/emit + checksum) round-trips a 4,104-byte bank, and a
  `DeviceControl` sends a voice and selects a program on the FM-1 over USB MIDI.
- The FM-1's USB audio is captured into the pool as a take (peaks + crash recovery), resampled and
  drift-compensated from 44.1 kHz into the session clock, and appears as a clip source in the
  arrangement.
- The determinism boundary (log = intent, take = ground truth) and the unlicensed prior-art
  disposition are recorded where the code lives.
- No `engine` change beyond implementing the already-declared seam.

## Risks

- **Latency and alignment.** The MIDI → hardware → USB-audio round trip offsets the captured take
  from the note frames. Measure and compensate a per-device fixed offset, or the arrangement will
  be subtly late. `DriftCompensator` covers *rate*, not this fixed *offset*.
- **44.1 kHz device vs 48 kHz session.** Capture must resample, and the device clock differs — a
  real quality/CPU decision; do not simply drop samples. (Same class as the Notepad capture, at a
  different rate.)
- **Determinism is per-artifact, not per-system.** The log replays; the *sound* does not. Guard the
  docs against implying byte-identical re-synthesis of hardware.
- **Licensing.** The prior-art repo has no license and bundles copyrighted Yamaha data; the DX7
  factory banks are Yamaha's. Code/data reuse is forbidden; independent implementation from
  documented formats is the path.
- **Protocol fragility.** FM-1 behaviour is firmware-dependent (its changelog tops out at V15), and
  bulk *readback* of stored voices is not documented — the librarian can push, but may not be able
  to pull from the device. Scope the feature to what the device actually exposes, and re-verify on
  firmware updates (the jam layer's grounding rules apply).
- **Profile sprawl.** Per-device specifics must not accrete into the core. Keep them in
  `DeviceControl`/profile configuration, and only for devices we own.

---

*Authored with DeepSeek-V4-Flash · DeepSeek Harness, 2026-09-12.*
