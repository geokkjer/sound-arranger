# Gear — Soundcraft Notepad-12FX (recording front-end)

> **Status: chosen** — the hardware mixer + multitrack-USB interface for the arranger's Phase 1 capture path.
> `groovebox/synth … → Notepad-12FX → USB (4 separate tracks) → laptop (recorder)`

## Why this box

The arranger records **live jams and improv**, so we need a physical mixer (hands-on level/EQ/send while playing) that also exposes each instrument as a **separate track over USB** — not just a stereo sum. The Notepad-12FX is the cheapest mixer that does genuine per-channel multitrack USB, and it matches the "4 tracks is enough" constraint.

Requirements it satisfies:

- ✅ Physical mixer (4 combo XLR/TRS inputs with preamps, Hi-Z, low-cut, 3-band EQ)
- ✅ **Multitrack USB** (4 separate capture channels into the laptop → separate WAVs)
- ✅ Small, cheap (~€150–180), powered, bus/line friendly
- ✅ Dub-friendly building blocks (post-fader aux send, built-in Lexicon delay/reverb)

## Verified specs

| Field | Value | Source |
|---|---|---|
| Audio Interface Input (to computer) | **4** | Thomann spec table |
| Audio Interface Output (from computer) | 2 | Thomann spec table |
| Multitrack Recording | **Yes** | Thomann spec table |
| USB/SD Direct Record | No | Thomann spec table |
| Mic/line inputs | 4 mono (combo XLR/TRS, 48 V phantom) | Thomann / manual |
| Stereo inputs | 2 (jack) + 1 (RCA) | Thomann |
| Aux sends | 1, post-fader (no pre-fader, no PFL) | Thomann |
| USB tap point | post-gain, **pre-EQ** (raw channel signal) | manual §7.0 |
| Class-compliant USB audio | yes (ALSA works with no driver) | soundcraft-utils README |

## USB routing — the part that matters

The 4 USB capture channels are **not** 4 freely-assignable inputs. Per the [soundcraft-utils README](https://github.com/lack/soundcraft-utils) (the authoritative Linux description of the device):

- **USB ch 1 & 2** = inputs **1 & 2**, fixed.
- **USB ch 3 & 4** = **Master L/R** *by default*, and routable to **input 3&4**, **input 5&6**, or **input 7&8**.

So the realistic track budget is **2 independent mono + one stereo pair = 4 tracks** (3 sources):

```
Instrument A (mono synth)   → input 1  → USB track 1
Instrument B (mono synth)   → input 2  → USB track 2
Instrument C (stereo groovebox) → inputs 3&4 → USB tracks 3 & 4
```

That maps cleanly onto the arranger: each USB channel is one non-destructive `Source` → `Clip`.

## Linux integration

- **Audio**: class-compliant USB device — ALSA sees all 4 channels out of the box (`arecord -l`), no vendor driver.
- **Routing**: the channel-routing control panel is Windows/Mac-only, but **[`soundcraft-utils`](https://github.com/lack/soundcraft-utils)** (Python, MIT) reproduces it on Linux. Supports exactly **Notepad-12FX / 8FX / 5**.

```bash
# Arch (current dev box)
yay -S soundcraft-utils

# CLI
soundcraft_ctl -l          # list routing choices
soundcraft_ctl -s <n>      # set routing (e.g. inputs 3&4 → USB ch 3&4)

# GUI
soundcraft_gui
```

Notes on `soundcraft-utils`:
- Uses a **D-Bus service** (`soundcraft_dbus_service --setup`) that runs as root to reach the USB device; the CLI/GUI stay unprivileged. Alternatively `--no-dbus` needs root/udev.
- Depends on **PyGObject** (`python3-gi` on Ubuntu / `python3-gobject` on Fedora).
- **NixOS**: experimental packaging at [`pakettiale/soundcraft-utils-nixos`](https://github.com/pakettiale/soundcraft-utils-nixos).
- Last commit 2021 (small, stable tool; ~36★) — fine for a fixed-function device, but confirm it still applies to the current firmware before relying on it.

## Native control (Rust) — the reverse-engineered protocol

`soundcraft-utils` is MIT, and its `soundcraft/notepad.py` contains the full reverse-engineered protocol — it's a **single vendor USB control transfer**, trivially portable to Rust. We can own the routing inside the arranger and drop the Python / PyGObject / D-Bus stack entirely.

- **Device IDs** (vendor `0x05FC` = HARMAN): Notepad-12FX `0x0032`, Notepad-8FX `0x0031`, Notepad-5 `0x0030`.
- **Set routing** — host→device vendor control transfer:
  - `bmRequestType = 0x40`, `bRequest = 16`, `wValue = 0`, `wIndex = 0`
  - payload (8 bytes) = `[0x00, 0x00, 0x04, 0x00, source, 0x00, 0x00, 0x00]` — only byte 4 (`source`) changes.
- **Routing sources (Notepad-12FX)** — applies to USB capture ch 3–4 (ch 1–2 are always Mic/Line 1 & 2):

  | value | source |
  |---|---|
  | 0 | Mic/Line 3 & 4 |
  | 1 | Stereo 5/6 |
  | 2 | Stereo 7/8 |
  | 3 | Mix (master L/R) — **default** |

- **Read side** (device→host: `0xA1`, bRequest `1`/`2`, `wValue 0x0100`, `wIndex 0x2900`, 256 B) exists upstream but is **not decoded yet** — not needed for routing.

**Rust plan**: enumerate with [`nusb`](https://crates.io/crates/nusb) (pure-Rust libusb replacement; `rusb` if you accept a C dep), send the 8-byte control transfer, and expose a `set_mixer_routing(source)` Tauri command + a dropdown in the UI. ~30 lines. Nice-to-have (routing is set-once), not Phase-1 critical.

## Caveats / gotchas

1. **Not "4 independent instruments"** — only inputs 1 & 2 are always-separate; tracks 3–4 are one stereo pair (or the master mix). Plan for 2 mono + 1 stereo, or re-route 3–4 as needed.
2. **Routing is set in software**, not on the panel — remember to set it (via `soundcraft_ctl`) or you'll silently record the master mix on 3–4.
3. **One aux, post-fader, no PFL** — fine for capture, but no per-channel solo and limited sends. Dub sends will live in software (our `fundsp` aux buses) anyway.
4. **Phantom power is always on** on the 4 mic/line channels — fine for synths/line, just don't hot-plug ribbon mics.
5. USB-B cable **not included**.

## Alternatives considered

| Option | USB to computer | Why not / why later |
|---|---|---|
| t.mix xmix 1202 FX USB (~€135) | 2-in/2-out stereo | No multitrack — out |
| Korg NTS-4 (~€180, DIY) | stereo 2-in/2-out | No multitrack — out |
| **Soundcraft Notepad-12FX (~€150–180)** | **4-in/2-out, multitrack** | **chosen** |
| Behringer Xenyx UFX1204 (~€350) | 16-in/4-out | Step up if we need >4 tracks |
| Zoom LiveTrak L-8 (~€400) | 12-in/4-out + SD recorder | Standalone jam recording; later |
| Tascam Model 12 (~€550–600) | 12-in/10-out + SD + MIDI | Overkill at this stage |

## Verification checklist (when the box arrives)

- [ ] `arecord -l` lists the Notepad with **4 capture channels** (class-compliant).
- [ ] `soundcraft_ctl -l` works; set USB ch 3–4 to the desired input pair.
- [ ] Record a 4-channel pass; confirm **4 separate WAVs**, no master-mix bleed on 3–4.
- [ ] `cpal` enumerates the device; pick the `hw:` card for the engine (see `docs/audio-latency.md`).
- [ ] Confirm pre-EQ tap: moving the channel EQ doesn't change the recorded signal.

## Sources

- Thomann spec table: `https://www.thomannmusic.no/soundcraft_notepad_12fx.htm` (Item 419464)
- Soundcraft Notepad manual (USB §7.0): `https://www.soundcraft.com/en/product_documents/notepadusermanual_v-1-0-pdf`
- `soundcraft-utils` (Linux routing): `https://github.com/lack/soundcraft-utils`
