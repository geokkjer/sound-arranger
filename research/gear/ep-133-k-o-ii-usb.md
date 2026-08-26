# Gear — Teenage Engineering EP-133 K.O. II (what's exposed over USB)

> **Status: research/characterization.** The unit is on the bus; this records exactly what the USB interface presents. Nothing is decision-locked yet — the "chosen" call (how the arranger talks to it) belongs to a note once we pick a path.
> **Device:** serial `EPEYS1BJ` · bcdDevice `0.00` · USB 2.00 **Full Speed (12 Mb/s)** · bus-powered 500 mA. **Two firmware snapshots:** pre-2.5 PID `2367:0020` (2 interfaces, MIDI-only — the baseline below) and **OS 2.5.1 PID `2367:8020` (4 interfaces — adds stereo USB audio)**, captured after the unit was updated.

## TL;DR

The EP-133 K.O. II over USB is **a class-compliant USB MIDI device and, since OS 2.5.1, a class-compliant stereo USB audio device (in *and* out)**. There is **no mass storage** (you can't mount it as a drive) and **no HID interface**. The interesting surface — samples, patterns, projects, the whole filesystem — is reached through the **undocumented SysEx protocol carried over that MIDI interface**, not through any standard USB class besides MIDI and audio.

## Baseline descriptor — pre-2.5 (`lsusb -v -d 2367:0020`, the unit's original firmware)

```
bcdUSB           2.00
bDeviceClass     0   [unknown — defined at interface level]
idVendor         2367 teenage engineering
idProduct        0020 EP-133
iManufacturer    teenage engineering
iProduct         EP-133
iSerial          EPEYS1BJ
bNumConfigurations 1

Configuration Descriptor (wTotalLength 0x0056, bus-powered, 500mA)
  Interface 0 — Audio, SubClass 0x01 Control Device, protocol 0, bNumEndpoints 0
      AudioControl HEADER  bcdADC 1.00 · bInCollection 1 · baInterfaceNr(0) 1
  Interface 1 — Audio, SubClass 0x03 MIDI Streaming, protocol 0, bNumEndpoints 2
      MIDI IN jack 1 · MIDI OUT jack 2
      EP 0x01 OUT  bulk, 64-byte packets
      EP 0x82 IN   bulk, 64-byte packets
```

`udevadm` confirms: `ID_USB_INTERFACES=:010100:010300:` (iface 0 subclass 0x01 audio control, iface 1 subclass 0x03 MIDI). The kernel driver is **`snd-usb-audio`** (`/sys/bus/usb/drivers/`), which binds the interface and creates an ALSA card.

### Current descriptor — OS 2.5.1 (`lsusb -v -d 2367:8020`)

Updating to **OS 2.5.1** re-enumerated the device under a **new PID `0x8020`** (the high bit is a firmware-version marker) with **4 interfaces**:

```
bNumInterfaces 4
  Interface 0 — Audio, Control Device (subclass 0x01), 0 endpoints
      AudioControl terminals:
        USB Streaming IN   (term 3, 2ch) → Speaker            (term 4)    [host → device: PLAYBACK]
        Digital Audio I/F  (term 5, 2ch) → USB Streaming OUT  (term 6)    [device engine → host: CAPTURE]
  Interface 1 — Audio, Streaming (subclass 0x02), AS_GENERAL→terminal 6   → CAPTURE (pcm0c)
      alt 0: 0 endpoints       alt 1: EP 0x83 IN  isoc, ASYNC, 2ch, 16-bit, 48 kHz, wMaxPacketSize 196 (implicit-feedback Data)
  Interface 2 — Audio, Streaming (subclass 0x02), AS_GENERAL→terminal 3   → PLAYBACK (pcm0p)
      alt 0: 0 endpoints       alt 1: EP 0x04 OUT isoc, ASYNC, 2ch, 16-bit, 48 kHz, wMaxPacketSize 196
                                     + EP 0x85 IN isoc feedback (wMaxPacketSize 4, bInterval 1)
  Interface 3 — Audio, MIDI Streaming (subclass 0x03), 2 endpoints (bulk, now 32-byte packets)
      EP 0x01 OUT · EP 0x82 IN    (MIDI IN jack 1 · MIDI OUT jack 2)
```

- **USB audio = stereo, 16-bit, 48 kHz, class-compliant** in both directions. The AS interfaces carry **0 endpoints at alt setting 0** and activate at **alt 1** (standard UAC structure).
- ALSA `stream0` confirms it: Playback = Interface 2, altset 1, `S16_LE`, 2ch, endpoint `0x04 OUT` (**ASYNC**, FL/FR), sync EP `0x85`; Capture = Interface 1, altset 1, `S16_LE`, 2ch, endpoint `0x83 IN` (**ASYNC**, FL/FR). Card 4 exposes `pcm0c` + `pcm0p` + `midi0`.
- **48 kHz** is the USB-audio rate; the device's *sample-engine* rate is separate ([46,875 Hz / 32 kHz / 26,250 Hz](https://teenage.engineering/guides/ep-133/whats-new)).
- MIDI bulk endpoints shrank from **64 B → 32 B** packets in 2.5.1.
- The USB MIDI raw port name is **`hw:4,0,0`** ("EP-133 MIDI 1"), found via `amidi -l`.

### Live capture on this unit (OS 2.5.1, [script](../../scripts/capture-ep133-usb.sh), 2026-08-25)

- **Audio capture confirmed.** `arecord -D hw:4,0` for ~6 s yielded **2ch / 16-bit / 48 kHz, 5.875 s** WAV with **real signal** (peak ≈ 3087 ≈ −20 dBFS, RMS ≈ 942) — the device outputs audio over USB, and the arranger's `Capture` path can record it.
- **MIDI idle = silent.** A 6 s `amidi -p hw:4,0,0 -d` dump captured **0 bytes** — the EP-133 emits nothing over USB MIDI when idle. To capture note/CC/SysEx, play pads/keys or run a pattern, or send a SysEx query. (The port is bidirectional; `amidi -l` shows `IO hw:4,0,0`.)


### What the baseline interfaces actually were

- **Interface 1 — the *real* surface: USB MIDI.** Two **bulk** endpoints (not isochronous), 64-byte packets each direction. This is the class-compliant MIDI path: 16 channels, and the pads/keys are presented as MIDI **notes**, not HID. It also carries the device's full **SysEx** protocol (see below).
- **Interface 0 — a *stub* USB audio control interface.** `AudioControl` with **zero endpoints**, `bcdADC 1.00`, and `bInCollection 1` pointing at interface 1. It exists so `snd-usb-audio` enumerates the device into a card at all, but it presents **no audio streaming interface** — no isochronous endpoints. It registers the device as `USB-Audio` but yields **no PCM streams**.

### What ALSA actually sees — baseline (pre-2.5, this host)

`/proc/asound/cards` → card **4 `[EP133]`** (USB-Audio), with a raw MIDI port at the time of the first capture:

```
/proc/asound/card4/id     EP133
/proc/asound/card4/midi0  Type: Legacy · Mode: native · Buffer 4096
  Output 0  Tx bytes: 139200    Input 0  Rx bytes: 2967435  Overruns: 0
/proc/asound/card4/usbid  2367:0020
```

- Baseline (pre-2.5): no `streams`/`pcmC*D*` → **no audio capture/playback streams**. On **2.5.1** card 4 gains `pcm0c` + `pcm0p` + `stream0` (see the current descriptor above).
- The **MIDI port is live** and already carrying traffic (Rx ≈ 2.97 MB in, Tx ≈ 139 KB out at time of capture) — the device streams MIDI to the host and accepts it back. The asymmetry (far more in than out) is consistent with a sampler/controller emitting a continuous stream.

> ⚠️ **This sandbox has no `/dev/snd` and no `/dev/bus/usb`** (the command sandbox is bubblewrap with `--dev /dev`, which mounts a fresh, empty devtmpfs — the host's device nodes are never bound in). So live byte-level capture (raw MIDI, SysEx, USB control transfers) was **not** possible here — only descriptor / sysfs / proc characterization and ALSA *proc* stats. A host with ALSA nodes can `amidi -l`, capture the port, and query the firmware over SysEx.

## What is exposed over USB

| Surface | Present? | How |
|---|---|---|
| **USB MIDI** | ✅ | Interface 1 (subclass 0x03), bulk EP 0x01 OUT + 0x82 IN, 64 B — class-compliant. ALSA raw MIDI port `EP133`. |
| **MIDI notes / control** | ✅ | 16-channel MIDI. 4 groups (A/B/C/D), 12 pads each mapped to notes: **A = 36–47 (C2–B2), B = 48–59 (C3–B3), C = 60–71 (C4–B4), D = 72–83 (C5–B5)**. Pad order A./A0/AFX/A1…A9 → offsets 0–11. Keys mode plays scales; CC/NRPN for project control. |
| **SysEx protocol (undocumented)** | ✅ (over MIDI iface) | Read/modify/generate/upload/verify full **projects and sample libraries** — pads, sequences, automation, scenes, songs, mixer, FX, sample metadata, sample library, sample upload. This is how TE's **EP Sample Tool** talks to it. |
| **Sample upload format** | ✅ (SysEx) | Device-native-rate **WAV** (KO-II standard rate **46,875 Hz**; lower recording rates 32 kHz / 26,250 Hz also supported) plus a JSON sidecar header: `{sound.playmode, sound.rootnote, sound.pitch, sound.pan, sound.amplitude, envelope.attack, envelope.release, time.mode}`. An `init` SysEx prefix precedes a transfer. |
| **USB *audio* (streaming)** | ✅ on OS 2.5.1 | Class-compliant **stereo 16-bit/48 kHz** in (capture EP `0x83`) and out (playback EP `0x04` + feedback EP `0x85`). **Pre-2.5: none** (no isochronous endpoints, no PCM). See the *Current descriptor — OS 2.5.1* section above. |
| **Mass storage / filesystem** | ❌ | Not a USB drive. The device filesystem is reachable **only** via SysEx (EP Sample Tool / community tools) or a `.pak`/`.ppak` backup archive made by the sample tool. |
| **HID** | ❌ | Pad/key input is MIDI notes, not a HID report. |

## The SysEx layer — the part worth knowing

Unofficial, **undocumented**, reverse-engineered by the community. Repos in the family:
[`kmorrill/ep-series-sysex`](https://github.com/kmorrill/ep-series-sysex) (EP-133 + EP-40, documentation-first, byte-level wire/project specs, reference Python), [`garrettjwilke/ep_133_sysex_thingy`](https://github.com/garrettjwilke/ep_133_sysex_thingy), [`wmealing/KO2-SYSEX`](https://github.com/wmealing/KO2-SYSEX), [`phones24/ep133-export-to-daw`](https://github.com/phones24/ep133-export-to-daw) (EP/EP-1320/EP-40 → Ableton/DAWproject/REAPER/MIDI), and the [`benjaminr/mcp-koii`](https://github.com/benjaminr/mcp-koii) MIDI controller (note/pattern playback over the MIDI port).

**Honest limits (from the `ep-series-sysex` author):**
- **System-menu settings are not writable over SysEx.**
- **Malformed writes can crash the device and require reformatting** — the tooling checkpoints and byte-verifies, and the docs carry a full error taxonomy learned from real incidents. **Back up projects and sounds via the EP Sample Tool (`.pak`/`.ppak`) before experimenting.**
- Not affiliated with TE; the EP Sample Tool is the reference implementation.

## Firmware 2.5 / 2.5.1 — the USB-audio delta

**OS 2.5** (free update; 2.5.1 is a follow-up, both ~2026) adds **class-compliant USB audio** — per TE [what's new](https://teenage.engineering/guides/ep-133/whats-new):
- **Stereo audio in**, both as a *sample source* and as a *live audio source* (plug into a phone/tablet and sample from any app).
- **Stereo USB audio out** to record straight into a DAW or any class-compliant host.
- Record sample-rate options: **HI 46,875 Hz** (standard), **MID 32 kHz**, **LO 26,250 Hz** (very lo-fi); mono sampling up to **40 s**.
- Also adds sample **reverse**, an **arpeggiator**, and **equal-length auto-chop**. ([Synth Anatomy](https://synthanatomy.com/2026/06/teenage-engineering-ep-133-k-o-ii-2-5-new-firmware-adds-usb-audio-reverse-and-more.html), [The Verge](https://www.theverge.com/entertainment/958723/teenage-engineering-os-25-ep-133-ko-ii-sampler).)

**On the unit here, OS 2.5.1 is confirmed live** — the descriptor now has the two AudioStreaming interfaces (stereo 16-bit / 48 kHz in & out), the PID moved to `0x8020`, and ALSA card 4 exposes `pcm0c`/`pcm0p`. USB audio is not aspirational for this device; it's present.

## Relevance to sound-arranger

The EP-133 is already a target in the project (the mixer's "4 groups"; the [EP-133 → DAW export tool](../architecture/2026-08-18-dawproject-and-ep133.md) as a project source via DAWproject).

- **Capture (audio):** on this unit (now OS 2.5.1, PID `0x8020`) the EP-133 itself is a **class-compliant stereo USB audio source/sink** (16-bit / 48 kHz), so the `Capture`/`CaptureNode` path can record it directly. The Phase-1 path is still the **Notepad-12FX** for multi-instrument mixing; the EP-133 can feed it via line/headphone out, **or** be captured as its own USB audio device.
- **MIDI input:** the engine's `midir` seam (Phase 5 / Phase 8 controls) can take the EP-133's USB MIDI directly — pads → notes 36–83 on 4 channels.
- **As a project/data source:** the **SysEx** protocol (or the `.pak` offline route) + the export tool → **DAWproject** → our import seam. This is a **genuine second-provider demand** for the project-import seam, same conclusion as the [dawproject research](../architecture/2026-08-18-dawproject-and-ep133.md).

## Caveats / gotchas

1. **Firmware-dependent for audio.** USB audio requires OS 2.5+; **2.5.1 is running here** (PID `0x8020`). Pre-2.5 the EP-133 is **MIDI-only** over USB.
2. **No mountable drive.** You can't copy samples off it like a USB stick; use the EP Sample Tool or SysEx.
3. **SysEx is undocumented and risky.** Treat it as reverse-engineered, non-affiliated; back up first, byte-verify writes.
4. **Full Speed (12 Mb/s)** — USB audio over it is fine for stereo but bandwidth is limited vs. High Speed.
5. **5-pin/MIDI out is separate** from USB; there's also a physical MIDI in/out (TRS) with its own sync behavior (e.g. continuous clock out), distinct from the USB MIDI port.

## Capturing it live

Run [`scripts/capture-ep133-usb.sh`](../../scripts/capture-ep133-usb.sh) **on the host** (not in this sandbox, which has no `/dev/snd` / `/dev/bus/usb`):

```bash
scripts/capture-ep133-usb.sh --seconds 6     # descriptor + ALSA + raw MIDI/SysEx + 6 s stereo capture
scripts/capture-ep133-usb.sh --play          # also send a 1 kHz test tone to the device's USB-audio OUT
scripts/capture-ep133-usb.sh --dry-run       # preview commands without executing
```

It writes a timestamped bundle under `.research/<vendor>-capture-*/` (gitignored; the EP-133 defaults to `2367-capture-*`) — `00_manifest.txt` summarises what it found and the resolved device / ALSA card / MIDI-port strings, plus a `00_run.log` of every command and its exit status. Requirements: `lsusb`, `udevadm`, `alsa-utils` (`amidi`/`aplay`/`arecord`); no sudo if your user is in the `audio` group. To capture audio, play pads on the EP-133 while it runs. (See the [capture-tooling note](../../.agents/notes/implemented/process/2026-08-25-ep133-usb-capture-tool.md).)

## Sources

- Live descriptor / sysfs / ALSA capture on the attached unit — baseline pre-2.5 (PID `2367:0020`) and **OS 2.5.1 (PID `2367:8020`)**. See the working capture at `/tmp/ep133_8020.txt` (session scratch; not committed).
- TE, "EP–133 guide: what's new" (OS 2.5/2.5.1 release notes; quoted USB-audio text): [teenage.engineering/guides/ep-133/whats-new](https://teenage.engineering/guides/ep-133/whats-new)
- TE, EP–133 downloads (firmware + EP Sample Tool): [teenage.engineering/downloads/ep-133](https://teenage.engineering/downloads/ep-133)
- Synth Anatomy, 2026-06-24, firmware 2.5 (USB audio, reverse, arpeggiator, sample rates, auto-chop): [synthanatomy.com](https://synthanatomy.com/2026/06/teenage-engineering-ep-133-k-o-ii-2-5-new-firmware-adds-usb-audio-reverse-and-more.html)
- The Verge, "Teenage Engineering adds lo-fi mode, USB audio…": [theverge.com](https://www.theverge.com/entertainment/958723/teenage-engineering-os-25-ep-133-ko-ii-sampler)
- `kmorrill/ep-series-sysex` (project/wire protocol docs; author's OP Forums write-up, "Opening up the EP series for third-party development"): [github.com/kmorrill/ep-series-sysex](https://github.com/kmorrill/ep-series-sysex) · [op-forums.com](https://op-forums.com/t/opening-up-the-ep-series-for-third-party-development/31759)
- `garrettjwilke/ep_133_sysex_thingy` (working `.syx`, sample-transfer format, WAV header, 46,875 Hz): [github.com/garrettjwilke/ep_133_sysex_thingy](https://github.com/garrettjwilke/ep_133_sysex_thingy)
- `wmealing/KO2-SYSEX` (SyEx reversing notes): [github.com/wmealing/KO2-SYSEX](https://github.com/wmealing/KO2-SYSEX)
- `phones24/ep133-export-to-daw` (EP-133 → Ableton / DAWproject / REAPER / MIDI): [github.com/phones24/ep133-export-to-daw](https://github.com/phones24/ep133-export-to-daw)
- `benjaminr/mcp-koii` (MIDI note/pad mapping A–D → 36–83): [github.com/benjaminr/mcp-koii](https://github.com/benjaminr/mcp-koii)
- EP Sample Tool usage: [teenagemanual.com](https://www.teenagemanual.com/ep-133/answers/use-the-ep-sample-tool)
