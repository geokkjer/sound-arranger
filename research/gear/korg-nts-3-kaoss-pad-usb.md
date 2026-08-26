# Gear — Korg NTS-3 kaoss pad kit (what's exposed over USB)

> **Status: research/characterization.** The unit is on the bus; this records exactly what the USB interface presents. Nothing is decision-locked yet.
> **Device:** KORG `0944:0153` NTS-3 kaoss pad kit · bcdDevice `1.00` · bcdUSB **1.10 Full Speed (12 Mb/s)** · **no serial number** (`iSerial 0`) · **1 configuration, 1 interface.**

## TL;DR

The NTS-3 kaoss pad is **pure class-compliant USB MIDI** over USB — **no USB audio, no mass storage, no HID**. It is a **programmable KAOSS XY-touch-pad effects unit** (Nu:Tekt DIY kit): the touchpad drives up to 4 effects at once, and its MIDI surface is both a **control surface** (XY-pad → CC) and an **editor/librarian channel** (KORG KONTROL Editor) for loading factory/custom effects. Audio lives on the **analog jacks**, not USB. **Firmware: latest is v1.4; the attached unit's `bcdDevice` is `1.00`, so it is likely behind — an update is available.**

## Device descriptor (captured `lsusb -v -d 0944:0153`)

```
bcdUSB           1.10
bDeviceClass     0   [unknown — defined at interface level]
idVendor         0944 KORG, Inc.
idProduct        0153 NTS-3 kaoss pad kit
bcdDevice        1.00
iManufacturer    KORG INC.
iProduct         NTS-3 kaoss pad kit
iSerial          0   (none)
bNumConfigurations 1   (wTotalLength 0x0073, bus-powered, MaxPower)

Configuration Descriptor
  Interface 0 — Audio, SubClass 0x03 MIDI Streaming, protocol 0, bNumEndpoints 2
      MIDIStreaming HEADER  bcdADC 1.00
      MIDI IN jack 16 · MIDI OUT jack 64   (embedded)
      MIDI IN jack 17 · MIDI OUT jack 65   (embedded)
      MIDI OUT jack 48 · MIDI IN jack 32   (external)
      MIDI OUT jack 49 · MIDI IN jack 33   (external)
      EP 0x01 OUT  bulk, 64-byte packets
      EP 0x82 IN   bulk, 64-byte packets
```

The interface is **MIDI Streaming only** (subclass 0x03) — it exposes **no Audio Streaming (isochronous) interface**, so **no audio over USB**. The two bulk endpoints (EP `0x01` OUT, EP `0x82` IN, 64 B) carry MIDI. The four MIDI-jack descriptor pairs (embedded + external) map to the device's **USB-MIDI** and its physical **MIDI in/out** (the NTS-3 also has DIN-style MIDI via its mini-jack).

### What ALSA actually sees (this host)

`/proc/asound/cards` → card **1 `[kit]`** — `KORG INC. NTS-3 kaoss pad kit` at usb-`…-1.3`, full speed. Its raw MIDI device:

```
/proc/asound/card1/midi0  (name "NTS-3 kaoss pad kit", Type: Legacy, Mode: native)
  Output 0 · Output 1      ← 2 MIDI OUT ports
  Input  0 · Input  1      ← 2 MIDI IN ports
```

The **2 in + 2 out** ports reflect the USB-MIDI and the physical MIDI paths. At capture time all counters were **0** (device idle; the sandbox also has no `/dev/snd` to drive it). The MIDI port is reachable on the host with `amidi -l` (the raw devices are `hw:1,0,0` / `hw:1,0,1`).

> ⚠️ Same sandbox limitation as the EP-133: no `/dev/snd` / `/dev/bus/usb` here, so characterisation is descriptor/sysfs/proc only. Live MIDI capture needs the host-side tool (below).

## What is exposed over USB

| Surface | Present? | How |
|---|---|---|
| **USB MIDI** | ✅ | Interface 0 (subclass 0x03), bulk EP `0x01` OUT + `0x82` IN, 64 B. ALSA raw MIDI port(s) on card 1 `[kit]`. |
| **MIDI controller** | ✅ | The KAOSS **XY touchpad** → MIDI CC (automatable X/Y), plus Mute / FX depth / tap-BPM / Latch driven from the pad. Full mapping in the [manual](https://manualzz.com/doc/html/78842392/korg-nts-3-kaoss-pad-kit-bedienungsanleitung). |
| **Editor / librarian** | ✅ (USB MIDI) | KORG **KONTROL Editor** (v2.5.1, 2025-06-10) manages effects/programs and loads custom user effects over USB MIDI. |
| **Programmable FX** | ✅ | Up to **4 effects** simultaneously, configurable routing (the KAOSS engine); open **logueSDK** (custom effects) loaded via the Librarian. |
| **USB *audio*** | ❌ | No isochronous streaming interface — audio is **not** exposed over USB. |
| **Mass storage / filesystem** | ❌ | Not a USB drive. |
| **HID** | ❌ | The X/Y pad is presented as **MIDI CC**, not a HID report. |

## Audio path (not USB)

The NTS-3 is an **effects unit** — audio goes in/out through its **analog jacks** (it processes a signal in the FX chain, with the XY pad controlling the effect). So for sound-arranger: **route its audio jacks into the Notepad-12FX** (the Phase-1 capture path), and treat USB as the **MIDI control + editor** channel only.

## Firmware — update check

- **Latest: System Updater **v1.4** (2025-04-17 build, announced 2025-05-12)**. Release notes: *reduced switching noise when continuously changing programs*, and a *logue-SDK bug fix in functions that access raw input*. Previous: **v1.3** (global-parameter-mono save bug, tempo-clock delay bug, logue-SDK unit-parameter display fix), **v1.2** (KONTROL EDITOR comms bugs, logue-SDK unit-parameter display fix). ([KORG JP downloads](https://www.korg.com/jp/support/download/software/0/934/5235/), [updates blog](https://www.korg.co.uk/blogs/updates/software-updates-for-nautilus-kross-2-and-nts-3-kaoss).)
- **Update route:** the official package is **Windows-only** — `NTS-3_kaoss_pad_kit_Updater_1_4_windows.zip` contains `kmupdate.exe` + the `*.VSB` firmware + `update_guide_windows_*.pdf`, and requires **Windows 10 22H2+** with the **KORG USB-MIDI Driver** installed first. So v1.4 is applied from Windows, not natively from Linux.
- **Installed version on this unit:** `bcdDevice` is `1.00`, which suggests it is **behind v1.4** — but `bcdDevice` is not authoritative for the Korg OS version, so **confirm on the device**: the NTS-3 shows its version on the touchpad display (check the manual's procedure, typically on boot), or read it from KORG KONTROL Editor.
- If you want v1.4: the **Windows updater + KORG USB-MIDI Driver** is the supported path. On Linux the official updater won't run; use a Windows machine/VM (or the owner's Windows box). Back up any custom effects (via KONTROL Editor) before an update.

## SDK scope — can it update firmware from Linux?

The NTS-3 is a **Nu:Tekt programmable** device with an open **logueSDK** (v2.0.0 for NTS-3, ARM Cortex-M7, min firmware ≥ v1.0.0). **The SDK is *not* a firmware tool** — it builds **user units** (custom oscillators/effects) as ELF shared objects, and KORG's [`logue-cli`](https://github.com/korginc/logue-sdk/tree/main/tools/logue-cli) loads them into the device's user-program slots. Its documented commands are **`check`, `probe`, `load`, `clear`** — **there is no `update`/`firmware`/flash command.** So:

- ✅ **From Linux, the SDK works for its purpose.** `logue-cli` has a Linux binary (`get_logue_cli_linux.sh`) and talks to the device over **USB MIDI** (card 1 `[kit]`); you can build and **load custom kaoss effects** from Linux (the [`schollz/logue`](https://github.com/schollz/logue) Docker/local workflow is a worked example).
- ❌ **The SDK is *not* how you update the OS firmware.** The `.VSB` OS image is flashed by KORG's **`kmupdate.exe`** (Windows) via the **KORG USB-MIDI Driver**. From Linux that needs a **Windows machine/VM** (Wine is unlikely to work: the updater needs the KORG USB-MIDI *kernel* driver, which Wine doesn't provide), or re-implementing the updater's undocumented bootloader protocol — unsupported and risky.

**Do you need the update?** The SDK only needs **≥ v1.0.0** on the NTS-3 (this unit's `bcdDevice 1.00` satisfies that), so SDK use isn't blocked. v1.4 buys *reduced noise when switching programs* and the *logue-SDK raw-input bug fix* — worth doing only if you hit those. To confirm the installed version, see the device display or KORG KONTROL Editor.

## Relevance to sound-arranger

- **Control surface:** the NTS-3's XY pad → MIDI CC is a great source of continuous automation/control for the engine (`ctx` control parameters), driven in real time by hand — a natural fit for the `midir` input seam (Phase 5 / Phase 8) and for recording CC as automation.
- **Effects/audio:** as a hardware FX box it sits **in the signal chain** (analog), so it's captured through the Notepad-12FX like any instrument — it doesn't add a USB audio device to the mixer.
- **Editor/API:** the open **logueSDK** means the NTS-3's effect palette is programmable; not directly a sound-arranger concern, but relevant if the owner wants generated/automated effects.

## Caveats / gotchas

1. **MIDI-only over USB** — no thumb-drive access, no audio streams; don't expect a mountable device.
2. **Firmware update needs Windows** (KORG USB-MIDI Driver + System Updater). `bcdDevice 1.00` suggests it's pre-1.4; confirm the installed version on the pad before deciding.
3. **Audio is analog** — to record it, use the NTS-3's audio out jacks into the front-end mixer, not USB.
4. **Full Speed (12 Mb/s)** and a single MIDI interface — plenty of headroom for MIDI CC, but USB is not a high-bandwidth path here.

## Sources

- Live descriptor / sysfs / ALSA capture on the attached unit (this run).
- KORG, "NTS-3 kaoss pad kit — PROGRAMMABLE EFFECT KIT" (product page; XY pad, 4 simultaneous effects, logueSDK, KONTROL Editor): [korg.com/sg/products/dj/nts_3](https://www.korg.com/sg/products/dj/nts_3/)
- KORG, System Updater v1.4 download (Windows, requires KORG USB-MIDI Driver): [korg.com/jp/support/download/software/0/934/5235](https://www.korg.com/jp/support/download/software/0/934/5235/)
- KORG UK updates blog, "Software Updates for Nautilus, Kross 2, and NTS-3 KAOSS" (v1.4 note): [korg.co.uk](https://www.korg.co.uk/blogs/updates/software-updates-for-nautilus-kross-2-and-nts-3-kaoss)
- KORG KONTROL Editor (v2.5.1) + NTS-3 editor/librarian: [korg.com/es/products/dj/nts_3/editor.php](https://www.korg.com/es/products/dj/nts_3/editor.php)
- `korginc/logue-sdk` (open SDK to build custom oscillators/effects; NTS-3 = SDK v2.0.0, ARM Cortex-M7, min firmware ≥ v1.0.0): [github.com/korginc/logue-sdk](https://github.com/korginc/logue-sdk)
- `logue-cli` (KORG tool; commands `check`/`probe`/`load`/`clear` for *user units*, Linux binary — no firmware command): [github.com/korginc/logue-sdk/tree/main/tools/logue-cli](https://github.com/korginc/logue-sdk/tree/main/tools/logue-cli)
- `schollz/logue` (Linux/Docker workflow to build and load logue units): [github.com/schollz/logue](https://github.com/schollz/logue)
- NTS-3 manual (36 pp; MIDI implementation chart): [manualzz.com](https://manualzz.com/doc/html/78842392/korg-nts-3-kaoss-pad-kit-bedienungsanleitung)
- Community: NTS-3 as a MIDI controller (XY → CC): [KVR / Bitwig forum](https://www.kvraudio.com/forum/viewtopic.php?f=259&t=618949) · [Elektronauts thread](https://www.elektronauts.com/t/korg-nts-3-kaoss-pad/207108/)
