#!/usr/bin/env bash
# Run the TUI under a pty and assert the *real terminal path* — the one
# TestBackend cannot cover: raw mode, the alternate screen, mouse capture, input
# decoding, a workflow key sequence reaching the app, and a clean restore on quit.
#
# This is the reproducible form of the check described in spikes/tui-shell/README.md.
# It needs `script` (util-linux) but no display server and no audio device: the
# host falls back to a silent pump when it cannot open the output.
#
#   ./scripts/pty-check.sh
set -euo pipefail

cd "$(dirname "$0")/.."
cargo build --quiet

log=$(mktemp)
work=$(mktemp -d)
trap 'rm -f "$log"; rm -rf "$work"' EXIT

# A session with something to act on: a generated pool source, one named clip and two
# markers. Generated here so the check needs no fixtures on disk.
python3 - "$work" <<'GEN'
import struct, sys, wave
d = sys.argv[1]
with wave.open(f"{d}/jam.wav", "wb") as w:
    w.setnchannels(1)
    w.setsampwidth(2)
    w.setframerate(48_000)
    w.writeframes(b"".join(
        struct.pack("<h", int(0.4 * 32767 * (((i // 2400) % 8) - 4) / 4))
        for i in range(48_000 * 4)
    ))
open(f"{d}/session.txt", "w").write(
    "host v1\nmount mixer channels=2 @0\n"
    f"pool {d}\n"
    "arrange add_track t0\n"
    "arrange add_clip t0 intro jam 0 96000 0 0 0 1.0 0 intro-take\n"
    "arrange set_marker 0 intro\n"
    "arrange set_marker 48000 chorus\n"
)
GEN

# Run 1 — the baseline shell path, with no session loaded (the demo snapshot, so the
# console has its full strip set): `bb` arms the snap grid at `beat` (a key the workflow
# owns, so this also proves the shared keymap reaches the terminal), then `q` quits.
# `stty` matters: `script` allocates a terminal with no size, and a zero-sized frame is a
# degenerate case the app must survive rather than exercise.
base=$(mktemp)
(sleep 3; printf 'bb'; sleep 1; printf 'q') \
  | timeout 30 script -qec "stty rows 30 cols 100; ./target/debug/tui-shell" "$base" \
      >/dev/null 2>&1 || true

# Run 2 — the alpha's *workflow* keys, on the loaded session: `;` jumps to the next
# marker (a workflow action reaching the terminal and mutating the transport), `x` splits
# under the playhead, `y`/`p` copy and paste it, `'` and `X` open the marker and export
# prompts (Esc cancels each), `q` quits.
(sleep 3; printf 'bb'; sleep 1; printf ';'; sleep 1; printf 'x'; sleep 1; printf 'yp'; \
 sleep 1; printf "'"; sleep 1; printf '\033'; sleep 1; printf 'X'; sleep 1; printf '\033'; \
 sleep 1; printf 'q') \
  | timeout 40 script -qec "stty rows 30 cols 100; ./target/debug/tui-shell --script $work/session.txt" "$log" \
      >/dev/null 2>&1 || true

fail=0

check() { # check <file> <needle> <description>
  if grep -qF "$2" "$1"; then
    printf 'ok   %s\n' "$3"
  else
    printf 'FAIL %s\n' "$3"
    fail=1
  fi
}

check_absent() {
  if grep -qF "$2" "$1"; then
    printf 'FAIL %s\n' "$3"
    fail=1
  else
    printf 'ok   %s\n' "$3"
  fi
}

# A phrase is **not contiguous** in the byte stream: ratatui writes a diff and
# skips cells that are already blank, so `grid off` arrives as `grid`, a cursor
# move, `off`. A tolerant regex is how a phrase is checked here; an exact
# `grep -F` is for single tokens (and the escape sequences themselves). `-z` puts
# the whole capture in one "line", so `.` also spans the newlines and cursor moves
# a phrase is broken across.
check_re() { # check_re <file> <extended regex> <description>
  if grep -zqE "$2" "$1"; then
    printf 'ok   %s\n' "$3"
  else
    printf 'FAIL %s\n' "$3"
    fail=1
  fi
}

check "$base" "$(printf '\033[?1049h')" "entered the alternate screen"
check "$base" "$(printf '\033[?1049l')" "restored the alternate screen"
check "$base" "$(printf '\033[?1000h')" "enabled mouse capture"
check "$base" "$(printf '\033[?1000l')" "released mouse capture"
check "$base" "spike"                   "drew the UI (title)"
check "$base" "master"                  "drew the meters"
check_re "$base" "snap.{0,40}gri"       "the grid key armed the snap grid (status line)"
check_re "$base" "grid.{0,30}off"       "the state line renders the grid"
check_absent "$base" "panicked"         "no panic (baseline)"

# The workflow run: single tokens only, because the phrase may be split by a cursor move
# (the caveat above). The *armed* state line and the bar/beat ruler are asserted by the
# render tests (`the_grid_cycles_from_the_keyboard_and_is_shown`,
# `the_ruler_draws_bars_and_beats_when_the_grid_is_armed`), which do not have to read a
# diff stream.
check "$log" "chorus"                   "the marker jump reached the next marker (its name)"
check_re "$log" "00:0[0-9]"             "and reported where it landed"
# The prefilled prompts draw into the header line. Only the op word is asserted — and
# without a trailing space, because ratatui skips cells that are already blank (the
# marker prompt's own text is written just when its cell changes, so asserting more here
# would assert the diff stream, not the app).
check_re "$log" "export"                "the export prompt opened (the prefilled line)"
check_absent "$log" "panicked"          "no panic (workflow run)"

if [ "$fail" -ne 0 ]; then
  echo "pty-check: FAILED (re-run the command in the README with the log path to inspect)"
fi

exit "$fail"
