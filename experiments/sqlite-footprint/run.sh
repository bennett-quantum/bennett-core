#!/usr/bin/env bash
# Trace, count and analyse one workload.
#   OUT=<build dir> RUNS=<runs dir> ./run.sh N QUERY
set -euo pipefail
: "${OUT:?set OUT to the build directory}"
: "${RUNS:?set RUNS to the directory for traces}"
HERE=$(cd "$(dirname "$0")" && pwd)
N=$1 Q=$2
D="$RUNS/$N/$Q"
rm -rf "$D"; mkdir -p "$D"
unset DISPLAY WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS BROWSER
cap=(systemd-run --user --scope -q -p MemoryMax=6G -p MemorySwapMax=0)
"${cap[@]}" "$OUT/driver_trace" "$N" "$Q" "$D"
mv "$D/meta.txt" "$D/meta.trace.txt"; mv "$D/rows.tsv" "$D/rows.trace.tsv"
"${cap[@]}" "$OUT/driver_perf" "$N" "$Q" "$D"
cmp -s "$D/rows.tsv" "$D/rows.trace.tsv" || { echo "plain and traced builds built different tables" >&2; exit 1; }
mv "$D/meta.txt" "$D/meta.perf.txt"; cp "$D/meta.trace.txt" "$D/meta.txt"
"${cap[@]}" python3 "$HERE/analyze.py" "$OUT" "$D" > "$D/analysis.txt"
echo "$D"
