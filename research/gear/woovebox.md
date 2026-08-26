# Gear — Woovebox (what's exposed, and the BLE-MIDI story)

> **Status: research/characterization + a resolved diagnosis (2026-08-26).** The Woovebox is present (BLE-connected, `WOOVEBOX-FA8E`). This records what's exposed and *why the web tool shows "not connected"* — a Linux BlueZ build issue, not a device problem.
> **Device:** Pocket Animal Audio **Woovebox** (micro music workstation / groovebox; OG 2023, SE, Pro — all firmware 2.0). **No USB data** — over USB it's **charging only**. Its real computer interface is **Bluetooth LE MIDI**.

## TL;DR

- **USB: charging-only.** There is **no USB data/MIDI** — the Woovebox's only computer data path is **BLE MIDI**.
- **BLE MIDI is exposed and works at the Bluetooth level** (paired/bonded/connected; advertises the standard MIDI-over-BLE UUID `03b80e5a-ede8-4b33-a751-6ce34ec4c700`). But on this host **Linux is not exposing it as an ALSA MIDI device**, so the *Web-MIDI-based* **Wooveconnect** web app sees no MIDI port → "not connected."
- **Root cause:** this distro's **BlueZ (5.87) was built without `--enable-midi`** (no `midi` plugin), so it doesn't publish BLE-MIDI peripherals to ALSA. This is **the "compiled without the midi flag" hint** in the Woovebox UI — it's about your OS Bluetooth stack, not the Woovebox.

## What's exposed over USB

| Surface | Present? | Notes |
|---|---|---|
| **USB data / MIDI / audio** | ❌ | **Charging only** (the site says "USB-C port (charging-only)"; USB-C cable **not** included). No data device enumerates — it's absent from `lsusb`. |
| **BLE MIDI** | ✅ | Standard **MIDI over Bluetooth LE** GATT service (`03b80e5a-…`); paired + bonded + connected. One BLE connection at a time. |
| **Audio** | ✅ (analog) | 3.5 mm stereo **out** (sync pulse via breakout cable, Pro), 3.5 mm stereo **in** (sampling / effects / live mix of external gear). |
| **MIDI (wired)** | ✅ (analog) | 3.5 mm **Type-A** MIDI in/out, switchable (not both); cable included with Pro. |
| **USB audio / mass storage / HID** | ❌ | None. |

## The BLE-MIDI → Web-MIDI chain (why "not connected")

The Woovebox web tool (Wooveconnect 2 — song/sample transfer, WAV export, firmware update) requires **"a MIDI connection + WebMIDI enabled browser"** — i.e. it uses the **Web MIDI API**, which on Linux is **backed by ALSA**. So even though the Woovebox pairs/connects at Bluetooth, the browser only "sees" it if the OS has created an **ALSA MIDI port** for it.

That port is created on Linux by **BlueZ's `midi` plugin** (built with `--enable-midi`). When it's missing:

```
Woovebox (BLE peripheral, connected) ──BlueZ──▶ [no midi plugin] ──▶ no ALSA MIDI port ──▶ Web MIDI enumerates nothing ──▶ Wooveconnect: "not connected"
```

**Confirmed on this host:** `bluetoothctl info C8:2E:18:EB:FA:8E` → `WOOVEBOX-FA8E`, `Paired: yes`, `Bonded: yes`, `Connected: yes`, UUID `03b80e5a-…`; but there's **no `midi` plugin** (`/usr/lib/bluetooth/plugins` absent) and **no BLE-MIDI ALSA card/port** (`/proc/asound` shows none). The device is fine — the Linux Bluetooth stack isn't publishing it to MIDI.

## Fixing it — make the Woovebox a real MIDI device

1. **Official (from the Woovebox Linux guide): build BlueZ with MIDI over BLE enabled.**
   ```
   sudo apt install libglib2.0-dev libudev-dev libical-dev libreadline-dev \
     libdbus-1-dev libasound2-dev build-essential python3-docutils
   cd /tmp && wget https://mirrors.edge.kernel.org/pub/linux/bluetooth/bluez-5.66.tar.xz
   tar -xf bluez-5.66.tar.xz && cd bluez-5.66
   ./configure --enable-midi --with-systemdsystemunitdir=/etc/systemd/system
   make && sudo make install && sudo apt-get install --reinstall bluez
   ```
   Reboot; then the Woovebox appears as a system-wide ALSA MIDI in/out device. (Adjust the BlueZ version / package manager to your distro — this box looks Arch/NixOS-like, so swap the `apt` commands accordingly.)
2. **Userspace BLE-MIDI → ALSA bridge** (no BlueZ rebuild): a small daemon that connects to the Woovebox's MIDI GATT and exposes an ALSA sequencer port (BLE-MIDI-peripheral → ALSA bridges exist in this niche; pick one that matches your distro's BlueZ/ALSA).
3. **Hardware adapter (the Woovebox site's "rock solid" pick): WIDI Bud Pro (CME)** — a USB BLE-MIDI dongle that presents the Woovebox as a **USB-MIDI** device. That's plug-and-play, needs no BLE host support, and Web MIDI sees it with no ALSA/BlueZ fiddling.

**Verify after the fix (on the host, has `/dev/snd`):** `aconnect -l` or `amidi -l` should show a BLE-MIDI port; then Chromium/Firefox Web MIDI enumerates the Woovebox and Wooveconnect connects. Confirm the browser supports Web MIDI (Chromium/Firefox do; iOS browsers don't, per Korg... per Woovebox), and that the user is in the `audio` group so the browser can open ALSA MIDI (geir already is).

## Project relevance — sound-arranger

- **Role:** the Woovebox is a **groovebox** (16-part synth, 16×16×16×16 sequencer, sampler, drum machine). For the arranger it's two things:
  - a **MIDI source** (notes/CC) — via BLE MIDI **once the ALSA port exists** (the fix above), into the engine's `midir`/control seam; and/or
  - an **audio source** — its **line out → Notepad-12FX** (USB is charging-only, so audio must be analog).
- **Caveats for a performance tool:** BLE MIDI **latency/reliability** varies with the OS Bluetooth stack (the Woovebox docs are candid about this). For stable use prefer the **3.5 mm TRS MIDI** (into a USB-MIDI interface) or a **WIDI Bud**; BLE is fine for control but a risk for tight sequencing. Only **one BLE connection** at a time, and battery-saver must be off.

## Caveats / gotchas

1. **USB is charging-only** — don't expect a mountable drive, USB audio, or USB MIDI.
2. **The web tool needs Web-MIDI + an ALSA MIDI port** — on Linux that's the BlueZ `midi` plugin (or a bridge/dongle). This was the whole "not connected" issue.
3. **BLE only** for wireless MIDI; stability is OS-stack-dependent.
4. **One BLE connection at a time**; turn off battery saver while connected.
5. **Browser**: WebMIDI-enabled (Chromium/Firefox on Linux); no iOS browser.

## Sources

- Woovebox product page (USB-C charging-only; BLE MIDI; audio/MIDI jacks; Wooveconnect 2 needs MIDI + WebMIDI browser): [woovebox.aife.me](https://woovebox.aife.me)
- Woovebox support — "Pairing your Woovebox: Linux" (the `--enable-midi` BlueZ fix for "compiled without MIDI over BLE support"): [woovebox.com](https://www.woovebox.com/support/guides--tutorials/wireless-midi/pairing-your-woovebox/linux)
- Woovebox support — "Pairing your Woovebox" (platform notes; WIDI Bud Pro recommendation; one-connection rule): [woovebox.com](https://www.woovebox.com/support/guides--tutorials/wireless-midi/pairing-your-woovebox)
- Live capture on this unit (2026-08-26): `bluetoothctl info`, `btmgmt`, `lsusb`, `/proc/asound` — device paired/bonded/connected with MIDI-over-BLE UUID; no BlueZ `midi` plugin, no ALSA MIDI port.
- BlueZ `--enable-midi` = the plugin that exposes BLE-MIDI peripherals to ALSA sequencer.
