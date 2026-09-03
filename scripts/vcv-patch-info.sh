#!/usr/bin/env bash
# vcv-patch-info.sh — summarize a VCV Rack patch (.vcv).
#
# VCV Rack 2 saves patches as a zstd-compressed tar containing patch.json (+
# modules/). Older/saved-plain patches may be JSON. This decompresses and prints
# the patch version, the module list, and the cable graph so you can see what a
# patch is without opening Rack (and correlate it with the CPU/RAM logger).
#
# Usage:   scripts/vcv-patch-info.sh [path/to.vcv]
#          (no arg => newest .vcv under ~/.local/share/Rack2/patches/)
#
# Requires: bash, zstd, tar, node.

set -euo pipefail

f="${1:-}"
if [ -z "$f" ]; then
  f="$(ls -1t "$HOME/.local/share/Rack2/patches/"*.vcv 2>/dev/null | head -1 || true)"
fi
if [ -z "$f" ] || [ ! -f "$f" ]; then
  echo "No patch found. Pass a path or save a patch in ~/.local/share/Rack2/patches/." >&2
  exit 1
fi
echo "Patch: $f"

magic="$(xxd -p -l 4 "$f" 2>/dev/null || true)"
if [ "$magic" = "28b52ffd" ]; then
  echo "Format: zstd-compressed tar"
  tmp="$(mktemp -d)"
  zstd -dc "$f" 2>/dev/null | tar -xf - -C "$tmp" 2>/dev/null
  pj="$tmp/patch.json"
  [ -f "$pj" ] || { echo "patch.json not found in archive" >&2; exit 1; }
else
  echo "Format: plain JSON"
  pj="$f"
fi

node -e '
const fs=require("fs");
const p=JSON.parse(fs.readFileSync(process.argv[1],"utf8"));
const id2m={};
(p.modules||[]).forEach(m=>id2m[m.id]={t:(m.plugin||"?")+"/"+(m.model||"?")});
console.log("version",p.version,"| modules",(p.modules||[]).length,"| cables",(p.cables||[]).length);
console.log("--- modules ---");
(p.modules||[]).forEach(m=>console.log("  ",m.id,m.plugin+"/"+m.model+"   pos="+JSON.stringify(m.pos)));
console.log("--- cables ---");
(p.cables||[]).forEach(c=>{
  const o=(id2m[c.outputModuleId]?.t||"?")+".o"+c.outputId;
  const i=(id2m[c.inputModuleId]?.t||"?")+".i"+c.inputId;
  console.log("  ",o,"->",i);
});
' "$pj"
