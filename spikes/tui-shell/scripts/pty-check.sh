#!/usr/bin/env bash
# Run the TUI under a pty and assert the *real terminal path* — the one
# TestBackend cannot cover: raw mode, the alternate screen, mouse capture, input
# decoding, and a clean restore on quit.
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
trap 'rm -f "$log"' EXIT

# `stty` inside the pty matters: `script` allocates a terminal with no size, and
# a zero-sized frame is a degenerate case the app must survive rather than
# exercise. The keys go in *after* the first frames: `bb` arms the snap grid at
# `beat` (a key the workflow owns, so this also proves the shared keymap reaches
# the terminal), then `q` quits.
(sleep 3; printf 'bb'; sleep 1; printf 'q') \
  | timeout 30 script -qec "stty rows 30 cols 100; ./target/debug/tui-shell" "$log" \
      >/dev/null 2>&1 || true

fail=0

check() { # check <needle> <description>
  if grep -qF "$1" "$log"; then
    printf 'ok   %s\n' "$2"
  else
    printf 'FAIL %s\n' "$2"
    fail=1
  fi
}

check_absent() {
  if grep -qF "$1" "$log"; then
    printf 'FAIL %s\n' "$2"
    fail=1
  else
    printf 'ok   %s\n' "$2"
  fi
}

# A phrase is **not contiguous** in the byte stream: ratatui writes a diff and
# skips cells that are already blank, so `grid off` arrives as `grid`, a cursor
# move, `off`. A tolerant regex is how a phrase is checked here; an exact
# `grep -F` is for single tokens (and the escape sequences themselves).
check_re() { # check_re <extended regex> <description>
  if grep -qE "$1" "$log"; then
    printf 'ok   %s\n' "$2"
  else
    printf 'FAIL %s\n' "$2"
    fail=1
  fi
}

check "$(printf '\033[?1049h')" "entered the alternate screen"
check "$(printf '\033[?1049l')" "restored the alternate screen"
check "$(printf '\033[?1000h')" "enabled mouse capture"
check "$(printf '\033[?1000l')" "released mouse capture"
check "spike"                   "drew the UI (title)"
check "master"                  "drew the meters"
check_re "snap.{0,40}gri"       "the grid key armed the snap grid (status line)"
check_re "grid.{0,30}off"       "the state line renders the grid"
# The *armed* state line and the bar/beat ruler are asserted by the render tests
# (`the_grid_cycles_from_the_keyboard_and_is_shown`,
# `the_ruler_draws_bars_and_beats_when_the_grid_is_armed`): in a diff stream only
# the changed cells are written, so `beat` arrives without the word `grid` next to
# it and a byte-level check would assert a cursor position, not a state.
check_absent "panicked"         "no panic"

if [ "$fail" -ne 0 ]; then
  echo "pty-check: FAILED (log kept at a temp path was removed; re-run the command in the README to inspect)"
fi

exit "$fail"
