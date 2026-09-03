# VCV Rack patches — the software sandbox for the Eurorack build

This folder holds **VCV Rack patch files (`.vcv`)** used to *prototype the planned hardware before buying* and to explore the generative / "studio as instrument" patching from the Autechre research. Each good patch here is a working model of a planned hardware configuration.

> See the [Eurorack GO rack plan](../research/gear/behringer-eurorack-go-rack-plan.md) for the hardware this software is sandboxing. Patches are the **software mirror** of that rack — and of the input/jam-layer "software as instrument" thesis.

## First reference voice (the intended hardware build)

The monophonic voice maps cleanly between software and the planned hardware:

| Software (VCV Rack) | Planned hardware | Role |
|---|---|---|
| **Vektor** (OhmerPrems) | Behringer **Victor** | quad / vector-morphing oscillator |
| **Venom** (VenomModules) | Behringer **Surges** | multimode filter |
| **Aestus** (Sanguine Mutants = Tides) | Behringer **Waves** | tidal modulator / function generator |
| **ADSR** (standard) | Behringer **140** | amplitude envelope |
| **VCA** (standard) | Rides-in-the-Storm **IDA** | amplitude |
| **Audio** (standard) | Behringer **CU1A** | audio out (USB) |

**The patch:** `MIDI>CV → Vektor (V/OCT + GATE) → Venom → VCA (CV from ADSR) → Audio`, with **Aestus (Tides)** driving **Vektor X/Y** (timbral crossfade) and/or **Venom cutoff** for the generative layer.

> ⚠️ Vektor's **GATE** drives its *internal MIX envelope* (a timed crossfade-automation of the 4 oscillator waveforms) and LFO retrigger — **it is not an amplitude envelope.** Amplitude comes from **ADSR → VCA**. For a static waveform blend, just set the joystick and leave Vektor's GATE unpatched.

## Naming & saving

- Save as `vcv-patch/<date>-<voice-or-concept>.vcv` (e.g. `2026-09-01-vektor-venom-aestus-monovoice.vcv`). Use a name that says what the patch *is*, not just its chain.
- **The `.vcv` is small and versionable.** VCV Rack 2 saves patches as a **zstd-compressed tar** (`patch.json` + `modules/`), so a committed `.vcv` is a few KB even with imported waveforms. The ~100 KB warning applies to the *uncompressed* `patch.json` (Vektor embeds each imported `.wav` at ~8 KB) — prefer the built-in ROM waveforms and keep the patch lean.
- **Inspect a patch without opening Rack:** `scripts/vcv-patch-info.sh [file.vcv]` decompresses the zstd tar and prints the version, module list, and cable graph — handy for correlating a patch with the CPU logger and for seeing what's in it without launching Rack.

## Measuring how much your machine can run

VCV Rack is **CPU-bound** (audio-rate DSP), not RAM-bound (patches are small in RAM unless you load heavy samples). The authoritative load meter is the **CPU bar in VCV Rack's top-right toolbar**. To log the whole-process cost over time so you can extrapolate how many voices/instances your machine handles:

```bash
scripts/log-vcv-usage.sh --duration 60 --interval 1   # sample a *running* Rack
```

It appends a **CSV** (with a header + per-interval rows: `cpu%`, `rss MB`, plus a total across the process tree) to `vcv-patch/.usage/` — **gitignored** — and prints a summary. `cpu%` can exceed 100 because Rack's audio engine multithreads; that's normal. See `scripts/log-vcv-usage.sh` for options.
