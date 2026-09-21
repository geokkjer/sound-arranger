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
# exercise.
(sleep 3; printf 'q') \
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

check "$(printf '\033[?1049h')" "entered the alternate screen"
check "$(printf '\033[?1049l')" "restored the alternate screen"
check "$(printf '\033[?1000h')" "enabled mouse capture"
check "$(printf '\033[?1000l')" "released mouse capture"
check "spike"                   "drew the UI (title)"
check "master"                  "drew the meters"
check_absent "panicked"         "no panic"

if [ "$fail" -ne 0 ]; then
  echo "pty-check: FAILED (log kept at a temp path was removed; re-run the command in the README to inspect)"
fi

exit "$fail"
