# Gear — M-VAVE FM-1 (what's exposed over USB)

> **Status: research/characterization (2026-08-26).** The FM-1 is on the bus. This records exactly what its USB composite interface presents and the firmware-update picture.
> **Device:** M-VAVE **FM-1** — a pocket **6-op FM** synthesizer (32 algorithms, 128 presets, 27 keys, arpeggiator, 16-step sequencer, battery + speaker). Enumerates as **`4c4a:c755` Jieli Technology USB Composite Device** · serial `4150353835313708` · bcdUSB **2.00 Full Speed (12 Mb/s)** · bcdDevice `1.00`.

## TL;DR

Unlike the EP-133 / NTS-3 / Woovebox, the FM-1 is a **native USB audio *and* MIDI** device:
- **USB audio** — stereo **24-bit / 44.1 kHz** **capture** (EP `0x83` IN: the synth's audio output → recordable) **and playback** (EP `0x02` OUT: host audio to the FM-1).
- **USB MIDI** — notes / CC / program-change over a bulk MIDI interface.
- **No HID interface in the active config** — the keys/knobs are presented as **MIDI**, not HID.
- ALSA sees it as card **1 `[Device]`** ("USB Composite Device") with **`pcm0c` + `pcm0p` + `midi0`** + `stream0`.

## Device descriptor (captured `lsusb -v -d 4c4a:c755`)

```
bcdUSB 2.00 · idVendor 0x4c4a (Jieli Technology) · idProduct 0xc755 · bcdDevice 1.00
serial 4150353835313708 · bNumConfigurations 1 (wTotalLength 0x0125)
bNumInterfaces 5 (bus-powered, Full Speed)

Interface 0 — Audio, SubClass 0x01 Control Device, 0 endpoints
    INPUT_TERMINAL 1  USB Streaming  (2ch)     ← host audio in
    INPUT_TERMINAL 4  Microphone     (2ch)
    OUTPUT_TERMINAL 3 Speaker (source 2)
    OUTPUT_TERMINAL 6 USB Streaming (source 7) ← device audio out (to host)
    SELECTOR_UNIT + FEATURE_UNITs
Interface 1 — Audio, SubClass 0x02 Streaming  → PLAYBACK (pcm0p)
    alt 1: EP 0x02 OUT isoc, 2ch, 24-bit, 44.1 kHz, wMaxPacketSize 288
Interface 2 — Audio, SubClass 0x02 Streaming  → CAPTURE (pcm0c)
    alt 1: EP 0x83 IN  isoc, 2ch, 24-bit, 44.1 kHz, wMaxPacketSize 288
Interface 3 — Audio, SubClass 0x01 Control Device, 0 endpoints (header 0x0009)
Interface 4 — Audio, SubClass 0x03 MIDI Streaming, 2 endpoints (bulk, 64 B)
    EP 0x84 IN · EP 0x04 OUT   (MIDI in + out)
```

- **USB audio = stereo, 24-bit, 44.1 kHz, class-compliant** in both directions. The **capture** path (EP `0x83`) is the FM-1's audio output, so the arranger's `Capture` can record it **directly over USB** — no analog mixer needed.
- **USB MIDI** interface 4 (bulk 64 B) carries note/CC/PC. The keys/knobs/display are MIDI, not HID.

### What ALSA sees (this host)

`/proc/asound/cards` → card **1 `[Device]`** — `Jieli Technology USB Composite Device` at usb-`…-1.3`, full speed. It exposes `pcm0c` (capture), `pcm0p` (playback), `midi0`, and `stream0`. So Linux enumerates it as both a **USB audio interface** and a **USB MIDI device** out of the box.

## What is exposed over USB

| Surface | Present? | How |
|---|---|---|
| **USB audio (capture)** | ✅ | EP `0x83` IN — stereo 24-bit/44.1 kHz, the synth's audio out. ALSA `pcm0c`. |
| **USB audio (playback)** | ✅ | EP `0x02` OUT — stereo 24-bit/44.1 kHz, host audio into the FM-1. ALSA `pcm0p`. |
| **USB MIDI** | ✅ | Interface 4, bulk 64 B in + out. Notes, CC (FX params, per the CC table), **PC 0–127** patch select, MIDI ch (default ALL). |
| **HID** | ❌ (in active config) | Controls are MIDI, not HID. (The firmware `.fwsc` references `usb_hid_ota.bin` — HID appears only for the OTA/bootloader update path.) |
| **Mass storage** | ❌ | Not a USB drive. |

## MIDI behaviour (from the firmware changelog)

The FM-1's [`FM-1.txt`](fm-1.txt) (KORG-style release notes, EN/CN) documents, up to **V15** (2026-07-30):
- **Note-on velocity 0 → note-off** (e.g. `90 30 00` treated as `80 30 00`).
- **CC responses** adjust FX/envelope parameters (now **0–100**), per the "CC response table".
- **MIDI channel** selectable in Glob (default **ALL**); **Program Change 0–127** selects patches; **BPM sync** for ARP/Seq.
- Many edit/sequencer additions (Glide, voice binding per pattern, chain mode, tone copy/rename, OP mute).

## Firmware — update picture

- The unit's **bcdDevice is `1.00`** (not authoritative for the app firmware). The supplied **`FM-1.fwsc`** is the update image; its contents reference **`usb_hid_ota.bin`** and **`*.ufw`**, i.e. the firmware is pushed over **USB HID OTA**.
- M-VAVE's updater is the **M-Upgrade** desktop tool (Windows/macOS). To apply firmware, connect the FM-1, run the updater, and it sends the `.fwsc` over USB-HID OTA. On Linux the official updater is Windows/Mac — plan on a Windows/Mac box (or check if M-Upgrade has a Linux/web path) if you want to update. The `.fwsc` here is safe to keep for that.
- Confirm the installed version first: the FM-1 shows its version on boot/display (check the manual), or read it from the M-Upgrade tool.

## Project relevance — sound-arranger

- **This is the first gear in the collection with native USB audio**, so it can be a **direct capture source** for the arranger: `Capture` reads card 1 at **stereo 24-bit/44.1 kHz** with no analog patching. For the dawless jamming/mixer setup, the FM-1's **line/audio out can also feed the Notepad-12FX** like the others — the choice is yours (USB-direct vs via-mixer).
- **MIDI:** the FM-1 is a USB MIDI source (notes/CC/PC) — it can drive the engine's `midir`/control seam and, via **PC 0–127**, change its own patches. Not needed for audio capture, but available if the owner wants to sequence/automate it.

## Caveats / gotchas

1. **USB audio is real (24-bit/44.1 kHz)** — unlike the MIDI-only gear, you don't strictly need the mixer for this one.
2. **Full Speed (12 Mb/s)** — fine for stereo 24-bit/44.1 kHz.
3. **Firmware** via **M-Upgrade (Windows/Mac), USB-HID OTA**. `bcdDevice 1.00` suggests a pre-V15 OS, but confirm on the device/updater before flashing.
4. **No HID** in normal operation — its controls are MIDI; don't expect a HID report descriptor for the knobs/keys.

## Sources

- Live descriptor / sysfs / ALSA capture on the attached unit (this run).
- M-VAVE **FM-1** product info (6-op FM, 32 alg, 128 presets, 27 keys, sequencer): [synthtopia](https://www.synthtopia.com/content/2026/07/13/m-vave-introduces-fm-1-fm-pocket-synthesizer/) · [midifan](https://www.midifan.com/m/news_body.php?id=59587) · [synthanatomy (V15 update)](https://synthanatomy.com/2026/07/m-vave-fm-1-a-budget-friendly-dx-7-style-desktop-fm-polysynth.html)
- M-VAVE **M-Upgrade** firmware tool (PC/Mac) + how to update: [cuvave.com.br](https://www.cuvave.com.br/blog/m-vave-m-upgrade-firmware-pc) · [cuvave.com.br (how to update)](https://www.cuvave.com.br/blog/firmware-m-vave-como-atualizar)
- Supplied on this machine: `research/gear/FM-1.fwsc` (update image; `usb_hid_ota.bin`/`*.ufw`) and `research/gear/FM-1.txt` (firmware release notes up to V15).
