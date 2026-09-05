# Behringer Eurorack — rack build plan

> **Status: in development · living document · no rush.**
>
> The working plan for the Eurorack rack — a **one-voice, self-evolving drone** in a compact desktop case. Not a characterization of attached hardware (contrast the [EP-133](ep-133-k-o-ii-usb.md) / [FM-1](m-vave-fm-1-usb.md) USB write-ups). Update the **Rack state** log as modules are ordered/racked; add/retire entries as the build evolves.

## Goal / context

- **Why:** explore *Eurorack* as the physical realization of the generative **"studio as instrument"** / Autechre-style patching (see the [Autechre generative-composition research](research/architecture/2026-09-01-autechre-generative-composition-inspiration.md)) — instead of buying more standalone synths/grooveboxes. The patch *is* the instrument; you rewire for each musical idea, and the rack **evolves itself** (no keyboard, no step-sequencer grid).
- **What it is:** a single analog voice (a complex oscillator) with a **modulation/randomness backbone** that moves it over time. **One voice, not a polysynth** — the drone's evolution comes from *timbre*, not from pitch/loudness sequencing.
- **Role in the rig:** the open, patchable generative source feeding → **Notepad-12FX** → arranger; also the **counterweight to closed proprietary gear** (the [input/jam-layer](.agents/notes/proposed/architecture/2026-09-01-input-jam-layer-device-registry-io.md) theme, in hardware).
- **FOSS / interop:** Eurorack is an open (if loose) standard — modular power rails, patch cables, 1V/oct, ~5–10V gates. Behringer modules are **analog, no firmware/updater/lock-in**. Caveat: verify gate polarity, CV range and 1V/oct between modules.

## The case

Tiptop Audio **Happy Ending Kit (Black EU)** — a simple single-row **84 HP** desktop frame with a **uZeus** power supply (two flying bus boards, external PSU). Cheap, portable, focused. **84 HP is exactly what this one-voice drone needs** — no wasted empty case, no carry weight.

- **Power:** uZeus **1000 mA +12V / 500 mA −12V**. The planned build draws **446 mA +12V / 200 mA −12V** — comfortable headroom.
- **Depth is the binding constraint.** This build maxes at **44 mm** (the Behringer 140 and 150 sit right at that limit — they're the tight ones). It's a single row, so no GO-style "top row ≈ bottom row" split; every module must be ≤44 mm.
- **Expandability:** the case is *full* (84 HP). Future modules mean a second row or a bigger case — a later decision, not this one.

## Software sandbox (VCV Rack)

Before buying, prototype in software. The voice is modelled as a VCV Rack patch in [`vcv-patch/`](../../vcv-patch/README.md) — the **software mirror** of this rack (and of the "software as instrument" thesis). The reference voice maps **Vektor ↔ Victor** (osc), **Venom ↔ Surges** (filter), **Aestus/Tides ↔ Waves** (modulator), **ADSR ↔ 140** (env), Audio ↔ CU1A.

Measure how much your machine can run with [`scripts/log-vcv-usage.sh`](../../scripts/log-vcv-usage.sh) (samples a running Rack's CPU%/RSS to `vcv-patch/.usage/`); Rack is CPU-bound, so that's the metric to watch.

## The build — one-voice evolving drone

A single analog voice whose timbre is continuously moved by a clocked-random / LFO backbone. The **audio path** is clean: **Victor → Surges → Verb → CU1A** (one source, one filter, one reverb, one USB out into the arranger).

| # | Module | HP | Depth | +12V | −12V | Role |
|---|---|---|---|---|---|---|
| 1 | Behringer **Victor** | 14 | 17 mm | 110 | 7 | **quad vector-morphing oscillator (the voice)** — 128 waveforms |
| 2 | Behringer **Surges** | 8 | 41 mm | 35 | 35 | four-pole "liquid" filter (Ripples clone) |
| 3 | Behringer **Waves** | 14 | 41 mm | 50 | 20 | function generator / LFO (Tides clone) |
| 4 | Behringer **140** | 16 | 44 mm | 40 | 40 | dual ADSR / LFO |
| 5 | Behringer **150** | 16 | 44 mm | 60 | 50 | ring mod / noise / S&H / LFO |
| 6 | Doepfer **A-148** | 4 | 30 mm | 20 | 20 | dual sample & hold |
| 7 | Doepfer **A-160-2** | 4 | 35 mm | 50 | 0 | clock / trigger divider |
| 8 | 2hp **Verb** | 2 | 42 mm | 81 | 28 | stereo reverb (the drone's space/tail) |
| 9 | Behringer **CU1A** | 6 | 36 mm | 0 | 0 | 2-in/2-out class-compliant USB audio → arranger |

**As racked: 84 HP, max depth 44 mm, 446 mA +12V / 200 mA −12V / 0 mA +5V.** HP is exactly full; power and depth are comfortably within the case.

- **CU1A note:** class-compliant USB audio (2-in/2-out, powered over USB — **0 mA from the rack**). Record the rack straight into the arranger over USB (like the FM-1). It's **AC-coupled (audio only)** — the *audio* bridge, not the CV bridge.
- **The modulation backbone** is what makes it *evolve*, not more voices: **A-160-2 + A-148** = clocked random CV; **150** (noise/S&H/LFO) into filter/pitch; **140 + Waves** = slow LFOs/function generators moving Victor's parameters. No keyboard, no MIDI-CV — the rack drives itself.

## The drone patch to start from

Patch **Waves (Tides) end-of-cycle → A-160-2 clock → A-148 trigger**, feed **A-148 CV → Victor pitch/parameter**, and **150 → S&H → Surges cutoff**, with the **150/140 LFOs** drifting Victor's vector position. Let it sit and slowly open the filter. That's the Autechre "not random, controlled and steered" idea in rack form.

## Generative techniques to explore (once racked)

- **Clock-driven S&H** — A-160-2 (divide) feeding A-148 (sample) into pitch/filter = slow, steppy, evolving CV.
- **Noise / S&H / ring** (150) — unvoiced-hybrid textures and clocked randomness.
- **LFO / function steering** (140, Waves) — drift and slow sweeps into the voice's timbre.
- **Filter-as-timbre** — since there's no VCA, the envelope is best used sweeping Surges' cutoff; the drone evolves by tonal movement, not loudness.
- **"Play the system"** — a manual intervention (NTS-3 XY pad) as the human-gated control, not full autonomy.

## Rig integration

- All audio → **Notepad-12FX** → arranger (analog, like the rest of the rig).
- The rack is the open, patchable, generative source — not another closed box with a Windows-only updater.
- **Not needed for this build:** a MIDI-CV bridge (the rack drives itself, per the steering direction) and a dedicated VCA (the drone wants sustained timbre, not amplitude swells).

## Deferred (not this build)

These were considered and are **parked, not part of the one-voice drone**:
- **More voices** (Behringer 110, mountable semi-modulars — East/West Beast, Mavis, 0-Coast). A second voice *is* the drone's feedstock, but this build deliberately starts with one voice + a modulation backbone.
- **A dedicated VCA** (for amplitude swells) — add later if the drone needs loudness motion, not now.
- **A quantizer / tuned-random** (e.g. Abacus/Marbles) for *musical* pitched motion — only if the drone should be tonal rather than atonal.
- **Sequencers / a MIDI-CV bridge** — the rack is self-driving; only revisit if you want the arranger or EP-133 to steer it.

## Rack state — living log (update as you build)

| Status | Module | HP/Dpth | Power (+12/−12) | Price | Acquired | Notes |
|---|---|---|---|---|---|---|
| planned | Tiptop Audio Happy Ending Kit (Black EU) case | 84 | 1000/500 | ~€130, verify | — | 1-row, uZeus + external PSU, ≤44 mm depth |
| planned (first build) | Behringer **Victor** | 14 / 17 | 110/7 | — | — | quad vector-morphing osc (voice) |
| planned (first build) | Behringer **Surges** | 8 / 41 | 35/35 | — | — | four-pole filter |
| planned (first build) | Behringer **Waves** | 14 / 41 | 50/20 | — | — | function gen / LFO |
| planned (first build) | Behringer **140** | 16 / 44 | 40/40 | — | — | dual ADSR / LFO |
| planned (first build) | Behringer **150** | 16 / 44 | 60/50 | — | — | ring / noise / S&H / LFO |
| planned (first build) | Doepfer **A-148** | 4 / 30 | 20/20 | — | — | dual S&H |
| planned (first build) | Doepfer **A-160-2** | 4 / 35 | 50/0 | — | — | clock divider |
| planned (first build) | 2hp **Verb** | 2 / 42 | 81/28 | — | — | stereo reverb |
| planned (first build) | Behringer **CU1A** | 6 / 36 | 0 mA | — | — | class-compliant USB audio → arranger |
| deferred | Behringer 110 | 16 / 46 | 80/50 | ~€110 | — | second voice — parked for this build |
| deferred | semi-modulars (East/West Beast, Mavis, 0-Coast) | — | — | — | — | more voices — parked |
| deferred | VCA (130 / IDA) · quantizer (Abacus) · MIDI-CV/ES-9 bridge | — | — | — | — | parked — not this build's direction |

*Fill per-module HP/depth/power from your ModularGrid and mark `Acquired` when it ships. Check [Thomann Norway](https://www.thomannmusic.no) for NOK incl. VAT.*

---

*Authored with DeepSeek-V4-Flash · deepseek-harness, 2026-09-03.*
