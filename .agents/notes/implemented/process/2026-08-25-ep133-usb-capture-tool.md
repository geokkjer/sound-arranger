# Agent Note: EP-133 USB host-side capture tooling

Status: implemented

## Problem

The sound-arranger agent needs raw capture of the attached Teenage Engineering EP-133 K.O. II (USB MIDI/SysEx, audio, descriptors) to confirm the protocol and drive the gear research. The command sandbox (bubblewrap, `--dev /dev`) mounts a fresh, empty devtmpfs: there is **no `/dev/snd` and no `/dev/bus/usb`** inside it, and the sandboxed process drops the host `audio` group. So the agent can characterise the device from descriptor/sysfs/proc but cannot perform a live capture itself. The device is on **OS 2.5.1** (PID `2367:8020`), exposing class-compliant stereo 16-bit/48 kHz USB audio in/out plus USB MIDI — the data worth capturing.

## Decision

Provide a **host-side capture script**, [`capture-ep133-usb.sh`](../../../../../music/music-composition-theory/studio/instruments/ep-133-k-o-ii/capture-ep133-usb.sh) (moved to the studio project), that the owner runs on the machine the device is attached to. It is read-only observation (no device writes), writes everything into a timestamped directory under **`.research/`** (already gitignored), which the sandbox can then read. It captures: USB descriptors (`lsusb -v` for every matched `VID:*` PID), udev/sysfs interface summary, ALSA state (`/proc/asound/cards`, `amidi -l`, `aplay -l`, `arecord -l`, `/proc/asound/card*`), a bounded raw MIDI/SysEx dump (`amidi -p <port> -d`), and (for audio-class devices) a stereo 16-bit/48 kHz capture (`arecord -D hw:C,0`), plus an optional playback probe (`--play`). It auto-detects the ALSA card by `/sys/class/sound/card*/id` and the MIDI port from `amidi -l` (falling back to `hw:C,M`), logs every command + exit status, and produces a `00_manifest.txt` line-up. `--dry-run` prints without executing. It is device-agnostic: `--vendor`/`--card-id` select the target (defaults: `2367`/`EP133` for the EP-133; the Korg NTS-3 is `0944`/`kit`).

## Alternatives considered

- **Grant the sandbox live device access** — add `--bind /dev/snd /dev/snd --bind /dev/bus/usb /dev/bus/usb` to `bwrapProfileArgs` in `debug/packages/sandbox/sandbox-local/src/profiles.ts` and give the process the `audio` group, then rebuild dsh and restart `dsh-web`. Rejected for now: it is a harness change, a rebuild, and the restart terminates the running web session, for a one-off capture.
- **`danger-full-access` / unconfined run** — the bash sandbox escalation covers file-path breadth, not device-node mounts, so it does not create `/dev/snd`. Rejected.
- **Host-side capture writing into the workspace** — chosen: zero dsh changes, the workspace is already bind-mounted rw into the sandbox, and the results are reusable later. Captures go to `.research/` so they are not committed.

## Consequences

- The script must be run on the host as a user in the `audio` group; outputs land in `.research/<vendor>-capture-<timestamp>/`.
- The agent reads the results back through the workspace bind. This keeps live capture out of the sandbox while still producing analysable data for the gear research and future engine work (MIDI/sysex protocol; the EP-133 as a stereo USB audio source on OS 2.5.1; the NTS-3 as a MIDI control surface).
