#!/usr/bin/env bash
# Capture a class-compliant USB MIDI/Audio device's USB surface for later analysis.
#
# This runs on the HOST (the machine the device is attached to) — NOT inside the
# agent sandbox, which has no /dev/snd or /dev/bus/usb. It dumps descriptors,
# ALSA state, raw MIDI/SysEx and (optionally) a stereo audio capture into a
# timestamped directory under .research/ so the results can be read back later.
#
# Usage:
#   scripts/capture-ep133-usb.sh [--out-dir DIR] [--seconds N] [--play] [--dry-run]
#                                [--vendor VID] [--card-id CARD_ID]
#
# Device selection (defaults target the Teenage Engineering EP-133 K.O. II):
#   --vendor VID     USB vendor id, e.g. 2367 (EP-133) or 0944 (Korg NTS-3)
#   --card-id ID     ALSA card id, e.g. EP133 or kit (NTS-3)
#   --out-dir DIR    Write into DIR (default: .research/<vendor>-capture-<timestamp>)
#   --seconds N      Live-capture duration in seconds (default 6)
#   --play           Also send a short test tone to the device's USB-audio OUT
#   --dry-run        Print the commands that would run; do not execute
#   -h, --help       Show this help
#
# Produces (in the out dir):
#   00_manifest.txt  summary + how to use it + detected device/card/port strings
#   00_run.log       every command and its exit status
#   01_lsusb.txt     USB descriptor(s)
#   02_sysfs.txt     udev/sysfs interface summary
#   03_alsa.txt      cards, amidi/aplay/arecord listings
#   04_alsa_proc.txt /proc/asound/card* details + rawmidi stream stats
#   05_raw_midi.bin  live MIDI/SysEx dump from the device's raw MIDI port
#   06_capture.wav   stereo 16-bit 48 kHz capture (device -> host), if present
#   07_replay.wav    test tone sent to the device (only with --play)
#
# Requirements: usbutils (lsusb), udevadm, alsa-utils (amidi/aplay/arecord).
# No sudo required if your user is in the `audio` group (or has a uaccess ACL).
# Nothing here writes to the device except the optional --play tone; the rest is
# read-only observation.

set -Eeuo pipefail

# ---------------------------------------------------------------------------
# Config / globals
# ---------------------------------------------------------------------------
PROG="$(basename "$0")"
OUT_DIR=""
SECONDS_DEFAULT=6
SECONDS_CAPTURE="$SECONDS_DEFAULT"
DO_PLAY=0
DRY_RUN=0
VENDOR="2367"
CARD_ID="EP133"

LOG=""          # set once OUT_DIR is known
MANIFEST=""

log() { # log <level> <msg>
  local level="$1"; shift
  local line
  line="$(date '+%H:%M:%S') [$level] $*"
  printf '%s\n' "$line" >> "${LOG:-/dev/null}"
  printf '%s\n' "$line" >&2
}

# run_cmd  <label> <outfile> <cmd...>   — tolerates failure, never aborts
run_cmd() {
  local label="$1" out="$2"; shift 2
  printf '\n===== %s =====\n' "$label" >> "${LOG:-/dev/null}"
  if [ "$DRY_RUN" -eq 1 ]; then
    log INFO "dry-run: ${label}: # $*"
    printf '%s\n' "[dry-run] $*" >> "$out"
    return 0
  fi
  if "$@" >>"$out" 2>>"$LOG"; then
    log INFO "${label}: OK"
    printf '%s\n' "exit: 0" >> "${LOG:-/dev/null}"
  else
    local rc=$?
    log WARN "${label}: exit ${rc} (see ${out} / log)"
  fi
}

usage() { sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'; exit 0; }

# ---------------------------------------------------------------------------
# Parse args
# ---------------------------------------------------------------------------
while [ "$#" -gt 0 ]; do
  case "$1" in
    --out-dir) OUT_DIR="${2:?--out-dir requires a path}"; shift 2 ;;
    --seconds) SECONDS_CAPTURE="${2:-$SECONDS_DEFAULT}"; shift 2 ;;
    --vendor)  VENDOR="${2:?--vendor requires an id}"; shift 2 ;;
    --card-id) CARD_ID="${2:?--card-id requires an id}"; shift 2 ;;
    --play)    DO_PLAY=1; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage ;;
    *) printf 'Unknown option: %s\n' "$1" >&2; usage ;;
  esac
done

# ---------------------------------------------------------------------------
# Dependency check
# ---------------------------------------------------------------------------
for tool in lsusb udevadm amidi aplay arecord; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    printf 'Missing required tool: %s (install usbutils / udevadm / alsa-utils)\n' "$tool" >&2
    exit 1
  fi
done

# ---------------------------------------------------------------------------
# Output dir
# ---------------------------------------------------------------------------
TS="$(date '+%Y%m%d-%H%M%S')"
if [ -z "$OUT_DIR" ]; then
  OUT_DIR=".research/${VENDOR}-capture-${TS}"
fi
mkdir -p "$OUT_DIR"
OUT_DIR="$(cd "$OUT_DIR" && pwd)"
LOG="$OUT_DIR/00_run.log"
MANIFEST="$OUT_DIR/00_manifest.txt"
: > "$LOG"

log INFO "EP-133 capture starting: out=$OUT_DIR secs=$SECONDS_CAPTURE play=$DO_PLAY"
printf 'EP-133 USB capture\nStarted: %s\nOut: %s\n' "$(date '+%Y-%m-%d %H:%M:%S %Z')" "$OUT_DIR" > "$MANIFEST"

# ---------------------------------------------------------------------------
# Detect the EP-133 on the USB bus and its ALSA card
# ---------------------------------------------------------------------------
# Card index: scan /sys/class/sound/card*/id for CARD_ID.
EP_CARD=""
for d in /sys/class/sound/card*; do
  if [ -f "$d/id" ] && [ "$(cat "$d/id" 2>/dev/null)" = "$CARD_ID" ]; then
    EP_CARD="${d##*card}"
    break
  fi
done

# USB devices (usually one; capture both 0x0020 and 0x8020 PIDs if present).
USB_LINES="$(lsusb 2>/dev/null | grep -i "${VENDOR}:" || true)"

{
  printf '\n== Detection ==\n'
  printf 'USB lines (vendor %s):\n%s\n' "$VENDOR" "$USB_LINES"
  printf 'ALSA card index for "%s": %s\n' "$CARD_ID" "${EP_CARD:-NOT FOUND}"
} >> "$MANIFEST"

log INFO "Detected ALSA card: '${EP_CARD:-none}'"

if [ -z "$EP_CARD" ]; then
  log WARN "No ALSA card with id '$CARD_ID' found. Audio/MIDI capture may fail; descriptor and sysfs will still be captured."
fi

# MIDI subdevice from /proc/asound/card<N>/midi<M> (e.g. card4/midi0 -> M=0).
EP_MIDI_SUB=""
if [ -n "$EP_CARD" ]; then
  EP_MIDI_SUB="$(ls /proc/asound/card${EP_CARD}/midi* 2>/dev/null | head -1 | xargs -r basename | sed 's/midi//')"
fi
# MIDI port from `amidi -l` output like:  IO  hw:4,0,0  EP-133 MIDI 1
# -> the rawmidi device string `hw:C,D,SUB`. Fall back to hw:C,M,0.
MIDI_PORT=""
AMIDI_LIST="$(amidi -l 2>/dev/null || true)"
MIDI_PORT="$(printf '%s\n' "$AMIDI_LIST" | awk -v c="$EP_CARD" '$2 ~ "^hw:" c "," {print $2; exit}')"
if [ -z "$MIDI_PORT" ] && [ -n "$EP_CARD" ] && [ -n "$EP_MIDI_SUB" ]; then
  MIDI_PORT="hw:${EP_CARD},${EP_MIDI_SUB},0"
fi
{
  printf '\n== MIDI ==\n'
  printf 'amidi -l:\n%s\n' "$AMIDI_LIST"
  printf 'EP-133 card=%s midisub=%s  midi port (resolved)=%s\n' \
    "${EP_CARD:-?}" "${EP_MIDI_SUB:-?}" "${MIDI_PORT:-unresolved}"
} >> "$MANIFEST"

# ---------------------------------------------------------------------------
# 1. USB descriptor(s)
# ---------------------------------------------------------------------------
run_cmd "lsusb (device list)" "$OUT_DIR/01_lsusb.txt" lsusb
if [ -n "$USB_LINES" ]; then
  while IFS= read -r line; do
    id="$(printf '%s\n' "$line" | awk '{print $6}')"
    [ -z "$id" ] && continue
    run_cmd "lsusb -v $id" "$OUT_DIR/01_lsusb.txt" lsusb -v -d "$id"
  done <<< "$USB_LINES"
fi

# ---------------------------------------------------------------------------
# 2. sysfs / udev
# ---------------------------------------------------------------------------
if [ -n "$USB_LINES" ]; then
  first_line="$(printf '%s\n' "$USB_LINES" | head -1)"
  ubus="$(printf '%s\n' "$first_line" | awk '{print $2}')"
  udev="$(printf '%s\n' "$first_line" | awk '{print $4}')"
  run_cmd "udevadm info (/dev/bus/usb/$ubus/$udev)" "$OUT_DIR/02_sysfs.txt" \
    udevadm info --query=all --name="/dev/bus/usb/$ubus/$udev"
fi
if [ -n "$EP_CARD" ]; then
  run_cmd "sysfs /proc/asound card$EP_CARD" "$OUT_DIR/02_sysfs.txt" \
    sh -c "ls -la /proc/asound/card${EP_CARD} 2>&1; echo; cat /proc/asound/card${EP_CARD}/usbid 2>&1; echo; cat /proc/asound/card${EP_CARD}/usbbus 2>&1"
fi
# interface class summary from sysfs (no /dev needed)
run_cmd "sysfs interface classes" "$OUT_DIR/02_sysfs.txt" \
  sh -c "for i in /sys/bus/usb/devices/*\:1.*/; do echo \"\$i class=\$(cat \$i/bInterfaceClass 2>/dev/null) sub=\$(cat \$i/bInterfaceSubClass 2>/dev/null) eps=\$(cat \$i/bNumEndpoints 2>/dev/null)\"; done"

# ---------------------------------------------------------------------------
# 3. ALSA listings
# ---------------------------------------------------------------------------
run_cmd "ALSA cards" "$OUT_DIR/03_alsa.txt" cat /proc/asound/cards
run_cmd "amidi -l" "$OUT_DIR/03_alsa.txt" amidi -l
run_cmd "aplay -l" "$OUT_DIR/03_alsa.txt" aplay -l
run_cmd "arecord -l" "$OUT_DIR/03_alsa.txt" arecord -l

# ---------------------------------------------------------------------------
# 4. /proc/asound detail
# ---------------------------------------------------------------------------
run_cmd "/proc/asound card$EP_CARD tree" "$OUT_DIR/04_alsa_proc.txt" \
  sh -c "find /proc/asound/card${EP_CARD} -maxdepth 1 2>/dev/null | sort | while read -r f; do echo \"--- \$f ---\"; cat \"\$f\" 2>&1; done"

# ---------------------------------------------------------------------------
# 5. Raw MIDI / SysEx dump (device -> host)
# ---------------------------------------------------------------------------
if [ -n "$MIDI_PORT" ]; then
  run_cmd "raw MIDI/SysEx dump (${SECONDS_CAPTURE}s on ${MIDI_PORT})" \
    "$OUT_DIR/05_raw_midi.bin" \
    timeout "${SECONDS_CAPTURE}s" amidi -p "$MIDI_PORT" -d
  # record how we invoked it for the manifest
  printf '\n== Raw MIDI ==\nPort: %s\nDump cmd: timeout %ss amidi -p %s -d\n' \
    "$MIDI_PORT" "$SECONDS_CAPTURE" "$MIDI_PORT" >> "$MANIFEST"
  ( printf 'Port: %s\n' "$MIDI_PORT"; printf 'Size: '; wc -c < "$OUT_DIR/05_raw_midi.bin" 2>/dev/null || echo 0 ) >> "$MANIFEST"
else
  log WARN "No MIDI port resolved; skipping raw-MIDI dump."
  printf '\nRaw MIDI: SKIPPED (no port resolved). See amidi -l above.\n' >> "$MANIFEST"
fi

# ---------------------------------------------------------------------------
# 6. Stereo audio capture (device -> host). Play something on the EP-133!
# ---------------------------------------------------------------------------
if [ -n "$EP_CARD" ]; then
  CAP_DEV="hw:${EP_CARD},0"
  WAV="$OUT_DIR/06_capture.wav"
  printf '\n===== audio capture %ss on %s =====\n' "$SECONDS_CAPTURE" "$CAP_DEV" >> "$LOG"
  if [ "$DRY_RUN" -eq 1 ]; then
    log INFO "dry-run: audio capture: # timeout ${SECONDS_CAPTURE}s arecord -D $CAP_DEV -f S16_LE -r 48000 -c 2 -t wav $WAV"
  elif timeout "${SECONDS_CAPTURE}s" arecord -D "$CAP_DEV" -f S16_LE -r 48000 -c 2 -t wav "$WAV" >>"$OUT_DIR/06_capture.txt" 2>>"$LOG"; then
    log INFO "audio capture: OK"
  else
    log WARN "audio capture: exit $? (details in $OUT_DIR/06_capture.txt / log)"
  fi
  printf '\n== Audio capture ==\nDevice: %s  (it records whatever the EP-133 outputs; play pads to get signal)\n' \
    "$CAP_DEV" >> "$MANIFEST"
else
  log WARN "No EP-133 ALSA card; skipping audio capture."
  printf '\nAudio capture: SKIPPED (no card)\n' >> "$MANIFEST"
fi

# ---------------------------------------------------------------------------
# 7. Playback probe (--play only)
# ---------------------------------------------------------------------------
if [ "$DO_PLAY" -eq 1 ]; then
  if [ -n "$EP_CARD" ]; then
    PB_DEV="hw:${EP_CARD},0"
    REF_WAV="$OUT_DIR/07_replay.wav"
    run_cmd "generate 1s 1kHz test tone" "$OUT_DIR/07_replay.txt" \
      sh -c "sox -n -r 48000 -c 2 -b 16 \"$REF_WAV\" synth 1 sine 1000 gain -6 2>&1 || true; ls -l \"$REF_WAV\" 2>&1 || echo 'sox missing; no tone generated (playback probe skipped)'"
    if [ -f "$REF_WAV" ]; then
      run_cmd "play test tone to ${PB_DEV}" "$OUT_DIR/07_replay.txt" \
        timeout 5 aplay -D "$PB_DEV" "$REF_WAV"
    else
      log INFO "no tone wav (sox optional) — skipping playback; did not send audio to the device."
    fi
    printf '\n== Playback probe ==\nDevice: %s\n' "$PB_DEV" >> "$MANIFEST"
  else
    log WARN "No EP-133 card; skipping playback probe."
  fi
fi

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------
{
  printf '\n== Finished ==\n%s\n' "$(date '+%Y-%m-%d %H:%M:%S %Z')"
  printf '\nFiles:\n'
  ( cd "$OUT_DIR" && ls -la )
} >> "$MANIFEST"

log INFO "Capture complete. Read: $MANIFEST"
printf '\nDone. Results in: %s\nOpen: %s\n' "$OUT_DIR" "$MANIFEST"
