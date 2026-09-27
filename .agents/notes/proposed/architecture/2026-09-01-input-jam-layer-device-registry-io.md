# Agent Note: Input/jam layer — device registry, normalized I/O, DeviceControl

Status: proposed

## Problem

Between the physical gear and the platform core sits an awkward, messy zone — what the [Synclavier design-inspiration review](../../../../research/architecture/2026-09-01-synclavier-design-inspiration-and-modular-hardware.md) calls the **input/jam layer**. The core (clock · graph interpreter · session log · context) is clean; the gear on the other end of the cable is not. The paper cuts already hit by the user's own rig research are all *interoperability* wounds, not capability gaps:

- **Firmware updaters are Windows/Mac-only** — KORG NTS-3 (Windows-only updater), M-Upgrade for the M-VAVE FM-1 (Windows/Mac). There is no Linux path and no open update story.
- **Closed/undocumented protocols reach your own data** — the EP-133's structured filesystem is reachable only via its undocumented SysEx; the Soundcraft Notepad-12FX routing is a vendor-defined USB protocol (`nusb`); KORG KONTROL Editor owns the NTS-3's channel; the FM-1 flashes over USB-HID OTA.
- **The OS publish layer betrays you** — the Woovebox "not connected" is a BlueZ built without `--enable-midi`, so its BLE-MIDI port never reaches ALSA and a vendor web UI breaks on Linux.
- **USB topology is unmodelled** — each device is a different combination (audio, MIDI, both, charging-only; class-compliant vs driver-bound) with no consistent answer to "what is this thing and what does it speak?"
- **Vendor lock-in** — each manufacturer owns the only path in: the updater, the protocol, and the UI.

The [Synclavier](../../../../research/architecture/2026-09-01-synclavier-design-inspiration-and-modular-hardware.md) is the apt counter-example — the ultimate closed system (proprietary everything, $25k–$200k, a terminal you couldn't script). The instinct here is the precise inverse: want **own** gear to be open, connected, recallable, scriptable across vendors. This note makes that instinct a first-class, but *bounded*, platform concern.

## Proposal

Make the **input/jam layer** a first-class concern of the platform, scoped strictly to **our own gear**, expressed as plugin seams over the minimal core — building the open, modular counterweight to the closed synth silo. Crucially, **this is an interoperability goal, not a compute goal** — the layered-out compute/RAM argument (no need for modular hardware as a compute fabric) stands unchanged.

- **`Device` / `Rig` registry plugin** — a declarative list of devices and roles (audio source, control surface, MIDI, CV, and how each connects). The app scans the attached rig (reusing the existing characterization tooling) and self-describes it. Very Cordis: a context service of known devices.
- **Normalized I/O seams** — `AudioIn/Out` (class-compliant + the `nusb` mixer routing + the FM-1's native USB audio), `MidiIn/Out` (USB **and** BLE-MIDI, including an explicit bridge for the BlueZ `--enable-midi` gap), `CvOut`/`SyncOut` for bridging to modular/semi-modular gear. *The capture-facing shape of a declared source — its identity, channels, clock role and binding rule — and the artifact model for recording a whole rig are decided in [the capture-topology note](2026-09-25-capture-topology-aligned-stems.md): this note owns the registry and the seams, that one owns what a capture produces and in what order the problems are solved.*
- **`DeviceControl` plugins** — small, reverse-engineered adapters that each solve *one* paper cut (EP-133 SysEx file access, Soundcraft routing, KORG editor channel, FM-1 HID-OTA), scoped to a device we own.
- **A Linux-friendly provision/update story** — flash from Linux over SysEx/HID where possible, at minimum a "don't brick it" path. A concrete, differentiated FOSS contribution.

**Explicitly out of scope:** a universal interop layer, universal device drivers, or emulating every vendor's tooling. The seam is the deliverable; per-device completeness is not. The grid-pad/control-surface layer (a sibling concern) drives both the rig and any hardware surface through this same normalized device model.

## Workload & grounding

This is the scope trap, called out up front. Building the input/jam layer is *fun* but unbounded if you let it be. The grounding rules:

- **Bounded to our own gear.** A device enters the registry/`DeviceControl` only because one of *our* workflows hit a paper cut. No stragglers, no "support the world," no completeness promise.
- **Time-box the reverse-engineering.** Each `DeviceControl` is scoped to a single paper cut; if a protocol is a rabbit hole, ship the seam and a documented manual path and move on.
- **The seam is the deliverable, not per-device coverage.** A normalized, recallable, scriptable device model + the I/O seams are the product value; any one device is replaceable/maintainable.
- **Keep it subordinate to the product.** The clip-arranger is the product; the input/jam layer serves it. Guard against drifting into a general-purpose audio-device middleware company.
- **The Synclavier bias, inverted.** Do not build a closed, monolithic path; every seam stays a plugin over the same core, so each piece is open, recallable, and replaceable.

## DIY / microcontroller avenue (deferred)

A separate, *not-required* avenue: **DIY synths and microcontrollers — e.g. the Electrosmith Daisy Seed** (STM32H7 + stereo codec, audio DSP). The user wants this flagged as worth exploring **if/when**, chiefly as a **learning experience** — soldering and building hardware — not as a product dependency. It is not blocked by the input/jam layer and does not block it; treat it as a hobby/learning track and an optional source of a discovery device for the `Device` registry. If it ever goes anywhere, it composes with the deferred ARM/Pi "box mode" (RESEARCH §8, Phase 5) and the existing `pi5-daisy-synth-rig` prior art. **Do not pull it forward on the critical path.**

## Alternatives considered

- **Build a universal interop layer / universal device driver set** — reimplements (or opposes) ALSA/JACK and every vendor's tooling; the surface is unbounded (every device is a proprietary island). Rejected; we own a narrow seam + our own gear, not the world.
- **Go modular (Eurorack-format) hardware *for compute*** — the only historical reason for this was that a 1980 CPU couldn't do real-time digital synthesis/sampling; DRAM went ~$4000/MB (1987) → ~$10/MB and a laptop runs it all in a `cpal` callback. No compute/RAM benefit. Rejected (unchanged).
- **Adopt the vendor's own tooling as the path** (KORG Editor, M-Upgrade, etc.) — the status quo we're escaping; Windows/Mac-only, closed, no Linux path. Rejected as the answer (still useful as documentation for reverse-engineering).
- **Build the input/jam layer now, broadly, before the product** — premature; the product (arranger) must exist before the interop layer has anything to serve. The layer follows the paper cuts, not the calendar.
- **Pull the Daisy Seed / DIY path onto the critical path now** — it is a learning/hardware track, not product. Deferred as if/when (see above).

## Acceptance criteria

- The platform has a `Device`/`Rig` registry that scans and self-describes attached gear, reusing the existing characterization tooling.
- Normalized I/O seams exist: `AudioIn/Out` (class-compliant + `nusb` + native USB audio), `MidiIn/Out` (USB + BLE, with a BlueZ bridge path), `CvOut`/`SyncOut`.
- `DeviceControl` plugin(s) exist that each solve one paper cut for a device we own; each is time-boxed and documented.
- A Linux-friendly firmware/provision path (or a "don't brick it" helper) exists for at least one device.
- The scope guardrails are recorded (this note) and followed: no universal interop, no universal drivers, per-device only on our own paper cuts, seam-first.
- The review doc and RESEARCH link to this note rather than restating it; RESEARCH §8/Phase 5 still marks ARM/Pi + hardware as deferred.

## Risks

- **Scope creep into a universal interop middleware** — the dominant risk; the "Workload & grounding" rules exist specifically to cap it.
- **Reverse-engineering is fragile** — closed protocols break on firmware updates; device descriptors change (the EP-133's USB ID changed on OS 2.5.1). A `DeviceControl` needs a re-verify path, not a "works forever" assumption.
- **Per-device maintenance burden** — each `DeviceControl` needs upkeep when a vendor ships a new firmware/protocol; keep the number small and only on our own paper cuts.
- **Value decay** — an interop layer's value erodes as vendors change things; it is a living body of work, not a static deliverable.
- **Distraction from the product** — the jam layer is valuable but not the product; guard its time budget and keep it subordinate to the clip-arranger.
- **Time on DIY/hardware is learning, not product** — the Daisy Seed track is optional and deferred; do not let a hobby pull the critical path.

*Authored with DeepSeek-V4-Flash · deepseek-harness, 2026-09-01.*
