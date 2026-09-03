#!/usr/bin/env bash
# log-vcv-usage.sh — sample the CPU% and RSS of a running VCV Rack instance.
#
# Purpose: correlate a VCV Rack patch with its resource cost so you can gauge how
# much module-DSP your machine can run. VCV Rack's own audio-engine load is the
# CPU bar in the top-right toolbar; this samples the whole Rack process (tree)
# CPU% and RSS over an interval so you can log it per patch.
#
# Usage:
#   scripts/log-vcv-usage.sh [--duration SEC] [--interval SEC]
#                            [--name PROCPATTERN] [--out FILE]
#
#   --duration  how long to sample (default 60)
#   --interval  seconds between samples (default 1)
#   --name      pgrep pattern for the Rack process (default: exact 'Rack')
#   --out       csv file to append (default vcv-patch/.usage/vcv-usage-<ts>.csv)
#
# Default is a CPU-bound, accurate per-interval measurement via /proc
# (so it needs Linux). cpu% can exceed 100 because Rack multithreads; that is
# expected. No external deps beyond bash + pgrep + getconf + awk.

set -euo pipefail

duration=60
interval=1
name='Rack'          # exact binary name by default (avoids matching this script)
out=""
ts="$(date -u +%Y%m%dT%H%M%SZ)"

while [ $# -gt 0 ]; do
  case "$1" in
    --duration) duration="$2"; shift 2 ;;
    --interval) interval="$2"; shift 2 ;;
    --name)     name="$2"; shift 2 ;;
    --out)      out="$2"; shift 2 ;;
    *) echo "unknown arg: $1" >&2; echo "usage: log-vcv-usage.sh [--duration SEC] [--interval SEC] [--name PROCPATTERN] [--out FILE]" >&2; exit 2 ;;
  esac
done

[ -z "$out" ] && out="vcv-patch/.usage/vcv-usage-${ts}.csv"
mkdir -p "$(dirname "$out")"

# Clock ticks per second, for /proc/<pid>/stat CPU accounting (Linux).
hz="$(getconf CLK_TCK 2>/dev/null || echo 100)"

# Find the Rack process(es). Always exact-name match (-x) so the script's own
# command line (which literally contains --name ...) is never matched.
if [ "$name" = "Rack" ]; then
  pids="$(pgrep -x Rack 2>/dev/null || true)"
  [ -z "$pids" ] && pids="$(pgrep -x rack 2>/dev/null || true)"
else
  pids="$(pgrep -x "$name" 2>/dev/null || true)"
fi
if [ -z "$pids" ]; then
  echo "No process matching '${name}' found. Start VCV Rack and open the patch first." >&2
  exit 1
fi
n="$(echo "$pids" | wc -w)"
[ "$n" -gt 1 ] && echo "Monitoring ${n} PIDs (Rack process tree): $(echo "$pids" | tr '\n' ' ')" >&2

cpu_ticks() { awk '{print $14+$15}' "/proc/$1/stat" 2>/dev/null || echo 0; }
rss_kb()    { awk '/VmRSS/{print $2}' "/proc/$1/status" 2>/dev/null || echo 0; }

# Header
echo "# vcv usage log -- $(date -u +%Y-%m-%dT%H:%M:%SZ)  pid(s)=$(echo "$pids" | tr '\n' ' ')  duration=${duration}s interval=${interval}s" | tee "$out"

# Snapshot of CPU ticks so the first interval is a true delta.
declare -A a
for p in $pids; do a[$p]=$(cpu_ticks "$p"); done

for ((i=0; i<duration; i+=interval)); do
  sleep "$interval"
  now="$(date -u +%H:%M:%S)"
  tot_cpu=0.0; tot_rss=0.0; row="$now"
  for p in $pids; do
    b="${a[$p]:-0}"
    t="$(cpu_ticks "$p")"
    cpu="$(awk -v d="$((t-b))" -v i="$interval" -v h="$hz" 'BEGIN{printf "%.1f", (d/h/i)*100}')"
    rss="$(rss_kb "$p")"
    rssmb="$(awk -v k="$rss" 'BEGIN{printf "%.1f", k/1024}')"
    tot_cpu="$(awk -v a="$tot_cpu" -v c="$cpu" 'BEGIN{printf "%.1f", a+c}')"
    tot_rss="$(awk -v a="$tot_rss" -v r="$rssmb" 'BEGIN{printf "%.1f", a+r}')"
    row="$row pid_${p}=${cpu}%,${rssmb}MB"
    a[$p]=$t
  done
  echo "$row TOTAL=${tot_cpu}%,${tot_rss}MB" | tee -a "$out"
done

echo "Logged to $out (${duration}s/${interval}s, ${n} pid(s)). cpu% is aggregate across the Rack tree." >&2
