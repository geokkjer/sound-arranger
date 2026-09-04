# Behringer Eurorack GO — rack build plan

> **Status: in development · living document · 1–2 year horizon, no rush.**
>
> This is the working plan for the Eurorack rack, not a characterization of attached hardware (contrast the [EP-133](ep-133-k-o-ii-usb.md) / [FM-1](m-vave-fm-1-usb.md) USB write-ups). Update the **Rack state** log as modules are ordered/racked; add/retire entries as the build evolves. No phase is fixed; treat it as a directional plan and adapt.

## Goal / context

- **Why:** explore *Eurorack* as the physical realization of the generative **"studio as instrument"** / Autechre-style patching (see the [Autechre generative-composition research](research/architecture/2026-09-01-autechre-generative-composition-inspiration.md)) — instead of buying more standalone synths/grooveboxes. The patch *is* the instrument; you rewire for each musical idea.
- **Role in the rig:** an analog voice + generative source feeding → **Notepad-12FX** → arranger. It's also the open, patchable **counterweight to closed proprietary gear** — the [input/jam-layer](.agents/notes/proposed/architecture/2026-09-01-input-jam-layer-device-registry-io.md) theme, in hardware.
- **FOSS / interop:** Eurorack is an open (if loose) standard — modular power rails, patch cables, 1V/oct, ~5–10V gates. Behringer modules are **analog, no firmware/updater/lock-in**. Caveat: it's a *loose* standard — verify gate polarity, CV range and 1V/oct between the mountable semi-modulars and the modules.

## The case

Behringer **Eurorack GO** — **2 × 140 HP = 280 HP**, **3 A power supply**, **32 power connectors**. Ample room and power for a layered system. Confirm per-module power draw (per rail) on [ModularGrid](https://www.modulargrid.net) before loading; keep headroom on the 3A PSU.

**Depth is the binding constraint, not HP.** The GO is a shallow, portable case: **top row ≤ 62 mm, lower row ≤ 40 mm** (the bottom deck is shallower). Rule of thumb: **deep modules up top, shallow ones down bottom** — sort every addition by depth before placing. Keep the top-row budget for the deep (complex-vco / DSP / function) modules and the bottom rows for utilities/VCA/CV. Depth per module is on ModularGrid.

## Software sandbox (VCV Rack)

Before buying, prototype in software. The planned voices are modelled as VCV Rack patches in [`vcv-patch/`](../../vcv-patch/README.md) — the **software mirror** of this rack (and of the "software as instrument" thesis). The reference monophonic voice maps Vektor ↔ Victor (osc), Venom ↔ Surges (filter), Aestus/Tides ↔ Waves (modulator), ADSR ↔ 140 (env), VCA ↔ IDA, Audio ↔ CU1A.

Measure how much your machine can run with [`scripts/log-vcv-usage.sh`](../../scripts/log-vcv-usage.sh) (samples a running Rack's CPU%/RSS to `vcv-patch/.usage/`); Rack is CPU-bound, so that's the metric to watch.

## Build strategy (staged, no rush)

### Phase 1 — voices (mountable semi-modulars)
Complete, patchable voices that **mount in the case** (Eurorack power + 1V/oct + mounting holes). Each is a finished voice (+ often a sequencer) rather than a bare VCO/VCA.

| Module | Flavor | ~price | Rack size | Notes |
|---|---|---|---|---|
| Cre8audio **East Beast** | East-coast (Moog-ish ladder filter) | ~$299 | rackable | voice + **32-step seq/arp** + MIDI/CV→CV, fully patchable — the East Beast from the monosynth decision |
| Cre8audio **West Pest** | West-coast (wavefolder + LPG) | ~$259 | rackable | the opposite flavor, same voice+seq+arp+CV architecture |
| Moog **Mavis** | Moog ladder filter | ~$349 | **44 HP** | compact self-contained Moog voice, patchable |
| Make Noise **0-Coast** | West/east hybrid (linear FM + lowpass gate) | ~$599 | rackable | the "complex"/lo-fi class (the "0-Coast" line); **0-CTRL** (~$299) is its touchpad/pressure sequencer |

### Phase 2 — the generative / modulation backbone (Behringer modules, cheap)
This is where the Autechre dynamics come from — modulation + randomness + sequencing, *not* another voice. **This is the steering direction:** the rack evolves through **semi-random modulation** (S&H, tuned random, cross-driving), *not* by driving it from external MIDI-CV. So Phase 2 beats Phase 3 for the "self-evolving" goal.

- **182 ×~2** — 16-step analog sequencers. **Buy two** for the cross-driving technique ("one sequencer drives the other's step position").
- **Abacus** — Behringer's tuned-random "music computer" (Mutable Instruments **Marbles** clone): random-but-musical, **steerable** CV/trigger. The heart of "not random but controlled."
- **140** — dual Envelope/LFO (fundamental modulation).
- **150** — Ring Mod / Noise / Sample & Hold (random CV + metallic timbres).
- **121 VCF** + **130 VCA** — filter and level platform.
- Utilities: a mixer (e.g. **173 Gate/Mixer** or a mixer module), a mult, an attenuverter.

### Phase 3 — output & the CV bridge (optional / later)
- Output mixing so the rack reaches the **Notepad-12FX** cleanly.
- **CV/sync bridge to the computer** — the "software mirror" hook (the input/jam-layer seam): Expert Sleepers **ES-8 / ES-9** (DC-coupled audio interface) or a MIDI/CV module. The semi-modulars already take MIDI + CV in, so you can sequence from the computer/arranger today.

## Planned first build (as spec'd on ModularGrid — the "good start")

A complete monophonic voice + headphone + direct USB audio, one row (~60 HP of 280): **Victor (osc) → Surges (filter) → IDA (VCA; envelope from 140/Waves) → CU1A (out/USB)**. Add a **second, self-contained voice** for layering/detuning: the **Behringer 110** (VCO/VCF/VCA in one module) — the drone's "build a stack of timbres" feedstock.

| # | Module | Role |
|---|---|---|
| 1 | Behringer **Victor** | quad / vector-morphing oscillator (voice) |
| 2 | Behringer **Surges** | "liquid" multimode filter |
| 3 | Behringer **140** | dual envelope + LFO (envelope, plus modulation) |
| 4 | Behringer **Waves** | function / "tidal" generator (extra envelope / modulation) |
| 5 | Rides in the Storm **IDA** | discrete VCA (amplitude) |
| 6 | Behringer **CU1A** | 2-in/2-out USB-C audio + headphone out — the **class-compliant USB-audio bridge to the arranger** |
| 7 | Behringer **110** | self-contained **VCO/VCF/VCA** voice — a second, detunable timbre to layer against Victor (drone depth) |

**As racked (Row 1): 76 HP, 335 mA +12 / 172 mA −12 / 0 mA +5, max depth 46 mm** — well within power (3A) and HP (280); the 44–46 mm modules go in the **top row** (≤62 mm).

- **First sounding patch:** Victor → Surges → IDA → CU1A, with **140** (or **Waves**) driving the IDA as an envelope, and the **110** layered behind as a second, detunable voice. CU1A gives you headphones *and* a direct USB-audio path into sound-arranger.
- **CU1A note:** class-compliant USB audio (48 kHz, 2-in/2-out, powered over USB — **0 mA from the rack**, ~$79). Record the rack straight into the arranger over USB (like the FM-1). It's **AC-coupled (audio only)** — so it's the *audio* bridge, **not** the CV bridge; the **Expert Sleepers ES-9** (DC-coupled) is the CV/sync bridge for the software mirror (watchlist).
- The **mountable semi-modulars** (East/West Beast, Mavis, 0-Coast) and the generative expansion (**182** sequencers, **Abacus**, **150**, **121/130**, utilities) remain the Phase 1/2 plan — watchlist below.

*Prices/HP per module are on your ModularGrid; check [Thomann Norway](https://www.thomannmusic.no) for NOK incl. VAT.*

## Generative techniques to explore (once racked)

- **Cross-driven sequencers** — two 182s, one driving the other's clock/step (the *Confield* trick).
- **Tuned random** — Abacus for random-but-musical CV/trigger, steered by inputs.
- **S&H / noise CV** — clocked random from the 150 into pitch/filter.
- **LFO/gate steering** — LFO into logic/gates to evolve patterns in time.
- **Semi-modular seq/arp** — use the East/West Beast's own sequencers live.
- **"Play the system"** — touchpad/pressure controllers (0-CTRL, or the NTS-3 XY pad) as the human-gated intervention Autechre use live; hands-on, not fully autonomous.

## Rig integration

- All audio → **Notepad-12FX** → arranger (analog, like the rest of the rig).
- MIDI + CV *in* from the computer/arranger today; **CV bridge** (ES-8/ES-9) later for the "software mirror."
- The rack is the open, patchable, generative source — not another closed box with a Windows-only updater.

## Rack state — living log (update as you build)

| Status | Module | HP/Dpth | Power (+12/−12) | Price | Acquired | Notes |
|---|---|---|---|---|---|---|
| planned | Behringer Eurorack GO case | 280 | — | — | — | 3A PSU, 32 connectors |
| planned (first build) | Behringer **Victor** | — | — | — | — | quad/vector osc (voice) — deep → top row |
| planned (first build) | Behringer **Surges** | — | — | — | — | liquid multimode filter |
| planned (first build) | Behringer **140** | — | — | ~$100+ | — | dual env/LFO |
| planned (first build) | Behringer **Waves** | — | — | — | — | function/tidal generator |
| planned (first build) | Rides in the Storm **IDA** | — | — | — | — | discrete VCA |
| planned (first build) | Behringer **CU1A** | 8 | 0 mA | ~$79 | — | headphone + class-compliant USB audio → arranger |
| planned (first build) | Behringer **110** | 16 | 80/+12 · 50/−12 | ~€110 | — | self-contained VCO/VCF/VCA — second voice, 46 mm → top row |
| watchlist | Cre8audio East Beast | rackable | — | ~$299 | — | voice + seq/arp (semi-modular) |
| watchlist | Cre8audio West Pest | rackable | — | ~$259 | — | west-coast voice |
| watchlist | Moog Mavis / Make Noise 0-Coast (+0-CTRL) | 44 | — | ~$349 / ~$599 | — | splurge tier |
| watchlist | Behringer 182 ×2 | ~16 ea | — | ~$149 ea | — | cross-driven sequencing |
| watchlist | Behringer Abacus | ~14–18 | — | ~$200 | — | tuned random |
| watchlist | Behringer 150 | ~16 | — | ~$99 | — | noise/S&H/ring |
| watchlist | Behringer 121 VCF / 130 VCA / mixer / mult | — | — | — | — | processing + utilities |
| watchlist | Expert Sleepers **ES-9** | — | — | — | — | **DC-coupled** CV/sync bridge (software mirror) |

*Fill per-module HP/depth/power from your ModularGrid and mark `Acquired` when it ships.*

---

*Authored with DeepSeek-V4-Flash · deepseek-harness, 2026-09-01.*
