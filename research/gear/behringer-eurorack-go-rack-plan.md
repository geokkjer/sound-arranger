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
This is where the Autechre dynamics come from — modulation + randomness + sequencing, *not* another voice.

- **182 ×~2** — 16-step analog sequencers. **Buy two** for the cross-driving technique ("one sequencer drives the other's step position").
- **Abacus** — Behringer's tuned-random "music computer" (Mutable Instruments **Marbles** clone): random-but-musical, **steerable** CV/trigger. The heart of "not random but controlled."
- **140** — dual Envelope/LFO (fundamental modulation).
- **150** — Ring Mod / Noise / Sample & Hold (random CV + metallic timbres).
- **121 VCF** + **130 VCA** — filter and level platform.
- Utilities: a mixer (e.g. **173 Gate/Mixer** or a mixer module), a mult, an attenuverter.

### Phase 3 — output & the CV bridge (optional / later)
- Output mixing so the rack reaches the **Notepad-12FX** cleanly.
- **CV/sync bridge to the computer** — the "software mirror" hook (the input/jam-layer seam): Expert Sleepers **ES-8 / ES-9** (DC-coupled audio interface) or a MIDI/CV module. The semi-modulars already take MIDI + CV in, so you can sequence from the computer/arranger today.

## Planned first build ("get music going now")

A coherent, patchable, generative core (~60–90 HP), leaving room to grow:

| # | Module | HP | ~price | Why |
|---|---|---|---|---|
| 1 | East Beast | rackable | ~$299 | east-coast voice + seq/arp + MIDI/CV |
| 2 | West Pest | rackable | ~$259 | west-coast voice, contrast |
| 3 | Behringer 182 (×2) | ~16 HP ea | ~$149 ea | cross-driven generative sequencing |
| 4 | Behringer Abacus | ~14–18 HP | ~$200 | tuned-random (generative heart) |
| 5 | Behringer 140 | ~24 HP | ~$100+ | dual env/LFO |
| 6 | Behringer 150 | ~16 HP | ~$99 | noise / S&H / ring |
| 7 | mult/attenuverter + mixer | ~8–16 HP | ~$60–150 | patch plumbing |

*Prices are approximate and region-vary — check [Thomann Norway](https://www.thomannmusic.no) for your price in NOK incl. VAT. HP is approximate — confirm on ModularGrid.*

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

| Status | Module | HP | Power (+12/−12) | Price | Acquired | Notes |
|---|---|---|---|---|---|---|
| planned | Behringer Eurorack GO case | 280 | — | — | — | 3A PSU, 32 connectors |
| planned | Cre8audio East Beast | rackable | — | ~$299 | — | |
| planned | Cre8audio West Pest | rackable | — | ~$259 | — | |
| planned | Behringer 182 ×2 | ~16 HP ea | — | ~$149 ea | — | cross-driving |
| planned | Behringer Abacus | ~14–18 HP | — | ~$200 | — | tuned random |
| planned | Behringer 140 | ~24 HP | — | ~$100+ | — | dual env/LFO |
| planned | Behringer 150 | ~16 HP | — | ~$99 | — | noise/S&H/ring |
| planned | mult/mixer utilities | — | — | — | — | patch plumbing |
| watchlist | Moog Mavis / 0-Coast (+0-CTRL) | 44 HP | — | ~$349 / ~$599 | — | splurge tier |
| watchlist | Expert Sleepers ES-8/ES-9 | — | — | — | — | CV bridge (later) |

---

*Authored with DeepSeek-V4-Flash · deepseek-harness, 2026-09-01.*
