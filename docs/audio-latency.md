# Audio latency on Linux & NixOS

> Research notes for **sound-arranger** — what kernel to run and how to tune userspace for low, *stable* latency.
> Kernel/package facts verified against `nixpkgs-unstable` on 2026-08-13 (§"verified" at the end).

## TL;DR

- **You do not need a "realtime kernel" for an arranger.** Record → cut → mix is not a hard-real-time workload.
- **The old advice ("install `linuxPackages_rt`") is dead in 2026**: PREEMPT_RT was **mainlined in kernel 6.12** (Nov 2024), and **nixpkgs removed the `-rt` kernel family** (verified: `linuxPackages-rt`, `-rt_latest`, `rt_6_1`, plus `lqx` and `hardened` are all "removed due to lack of maintenance").
- **Run the stock kernel** (`linuxPackages_latest`, currently **7.1.8**) — or `linuxPackages_zen` / `linuxPackages_xanmod` if you want a bit more desktop low-latency tuning — and spend your effort on the **userspace** settings that matter more: `rtkit`, `performance` governor, and talking to **ALSA `hw:` directly** from `cpal`.
- Hard real-time (sub-3 ms, live soft-synth through plugins) is the *only* case that justifies building `CONFIG_PREEMPT_RT=y` yourself. Defer it.

## The latency model

Round-trip latency ≈ buffer + scheduler wakeup + device/driver + any software mixer.

| Term | Typical | Where it's set |
|---|---|---|
| Buffer | 128–256 frames @ 48 kHz = **2.7–5.3 ms** | app / cpal / PipeWire quantum |
| Scheduler wakeup (jitter) | 0.05–1 ms | RT scheduling + kernel preemption |
| Device/driver | 1–5 ms | audio interface |
| Software mixer | 0–10 ms | PipeWire/Pulse if not using ALSA `hw:` |

**Jitter** (variance) causes xruns/clicks far more than average latency. RT scheduling + a pinned CPU governor + threaded IRQs attack jitter directly.

## The kernel question (verified 2026)

PREEMPT_RT merged into mainline in **6.12**; a modern kernel already *contains* the RT machinery — it's just a build-time option, not a separate patchset. In current nixpkgs:

| Kernel set | Version | Notes |
|---|---|---|
| `linuxPackages_latest` | **7.1.8** | Default; fine for audio |
| `linuxPackages_zen` | 7.1.8 | Tuned for desktop responsiveness |
| `linuxPackages_xanmod` / `_latest` / `_stable` | 6.18.44 / 7.1.8 / 7.1.8 | Low-latency + performance tweaks |
| `linuxPackages_6_12` | 6.12.103 | LTS (this is the RT-mainline baseline) |
| `linuxPackages-rt`, `-rt_latest`, `rt_6_1` | **REMOVED** | no longer maintained in nixpkgs |
| `linuxPackages_lqx`, `linuxPackages_hardened` | **REMOVED** | no longer maintained in nixpkgs |

**Recommendation:**
- **Default / dev / arranger:** `linuxPackages_latest` (or `zen`). No RT needed.
- **Want lower jitter on a desktop without full RT:** `linuxPackages_xanmod` or `linuxPackages_zen`.
- **Hard real-time (live soft-synth, sub-3 ms):** build your own kernel with `CONFIG_PREEMPT_RT=y` via `structuredExtraConfig` (RT is mainlined, so no external patch). Recipe varies by nixpkgs — only do this if you actually need it.

## Userspace tuning (matters more than the kernel)

1. **RT scheduling + memlock** for the audio process — on modern systems this is handled by **`rtkit`** (PipeWire/WirePlumber request it automatically). This replaces the old hand-edited `/etc/security/limits.d/audio.conf`.
2. **CPU governor = `performance`** (or disable frequency scaling) — avoids the ~100 µs–ms wake-up cost of ramping a core from a low P-state.
3. **`threadirqs`** — threaded interrupt handlers (usually the default now); optionally give the audio interface's IRQ higher priority with `rtirq`.
4. **ALSA `hw:` device** — bypass the PipeWire/Pulse mixer entirely for the lowest, most deterministic path.

## NixOS configuration.nix (recommended)

```nix
{ config, pkgs, lib, ... }: {
  # --- kernel ---
  boot.kernelPackages = pkgs.linuxPackages_latest;   # or pkgs.linuxPackages_zen
  boot.kernelParams = [ "threadirqs" ];              # threaded IRQs (usually default already)
  powerManagement.cpuFreqGovernor = "performance";

  # --- audio: rtkit grants RT + memlock to audio processes (modern limits.d) ---
  security.rtkit.enable = true;

  # --- PipeWire (ALSA / Pulse / JACK compat) ---
  services.pipewire = {
    enable = true;
    audio.enable = true;
    alsa.enable = true;          # route ALSA apps through PipeWire
    alsa.support32Bit = true;
    pulse.enable = true;         # PulseAudio compatibility
    jack.enable = true;          # JACK compatibility (inter-app routing)
  };
  hardware.pulseaudio.enable = false;

  # your user can touch audio hardware
  users.users.YOURUSER.extraGroups = [ "audio" ];
}
```

- **rtkit is the key line** — without it you'd hand-edit limits (`@audio - rtprio 95`, `@audio - memlock unlimited`). rtkit is the supported path on modern NixOS.
- **Lower PipeWire latency** (if you route through PipeWire instead of ALSA `hw:`): set a small quantum via WirePlumber, e.g. `default.clock.quantum = 128` (or `node.latency = 128/48000`). Leave the default for normal desktop use.

## Arch Linux (your current dev box)

- Kernels: `linux` (default), `linux-lts`, `linux-zen` (low-latency), and `linux-rt` / `linux-rt-lts` (official repos). Prefer `linux` or `linux-zen` for the prototype.
- `pipewire` + `wireplumber` + `pipewire-alsa` + `pipewire-pulse` + `pipewire-jack` + `rtkit`.
- Pin the governor: `sudo cpupower frequency-set -g performance` (or use `power-profiles-daemon`).
- `rtirq` (optional) to raise the audio IRQ priority; `realtime-privileges` (AUR) as an rtkit alternative.

## cpal / engine integration

- `cpal`'s default Linux backend is **ALSA**. For lowest latency, enumerate devices and open the hardware device directly (the `hw:` card) rather than the `default`/`pulse` alias, so you skip PipeWire/Pulse mixing. Keep a `default` fallback for machines without a raw device.
- Bump the callback thread to realtime priority with the **`audio_thread_priority`** crate (on Linux it needs rtkit or limits to succeed — hence the NixOS/Arch config above).
- Start at **512 frames**, drop to **256 / 128** as it stabilizes. For an arranger (record + playback), 256 is a comfortable target.

## Measuring

- `pw-top` — PipeWire quantum + DSP load + xrun count.
- `jack_iodelay` — actual round-trip latency through the JACK/PipeWire graph.
- `aplay` / `arecord` — smoke-test device I/O.
- `cyclictest` (from the `rt-tests` package) — measure kernel scheduling latency/jitter if you ever go RT.

## Recommendation for this project

An arranger records, edits and mixes — it does **not** run a live soft-synth through realtime plugins. So: **stock `linuxPackages_latest` + `rtkit` + `performance` governor + ALSA `hw:` from cpal** is plenty, and it's the lowest-maintenance choice. Revisit the (custom-built) `PREEMPT_RT` kernel only if Phase 2+ adds live performance/monitoring through DSP and you measure xruns.

---

### Verified (nixpkgs-unstable, 2026-08-13)

- Kernel versions: `linuxPackages_latest` = 7.1.8, `linuxPackages_7_1` = 7.1.8, `linuxPackages_6_12` = 6.12.103, `linuxPackages_xanmod` = 6.18.44, `xanmod_latest` = 7.1.8, `xanmod_stable` = 7.1.8, `linuxPackages_zen` = 7.1.8.
- Removed from nixpkgs ("lack of maintenance"): `linuxPackages-rt`, `linuxPackages-rt_latest`, `linuxPackages_rt_6_1` (and the older `rt_5_*` family), `linuxPackages_lqx`, `linuxPackages_hardened`.
- Package versions: `nodejs_22` 22.23.2, `webkitgtk_4_1` 2.52.5, `alsa-lib` 1.2.16.1, `jack2` 1.9.22.
- PREEMPT_RT mainlined in kernel 6.12 (Nov 2024).
