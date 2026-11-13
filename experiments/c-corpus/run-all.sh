#!/bin/sh
# Runs every top-level test of a corpus directory through run-one.sh, in parallel, and collects results.
#
#   run-all.sh <corpus dir> <work dir> [jobs]
#
# Uses the binaries copied to BIN (default: <work dir>/../bin, never the live build tree). BUDGET and STACK may be
# overridden.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
corpus=$(cd "$1" && pwd)
WORK=$(mkdir -p "$2" && cd "$2" && pwd)
jobs=${3:-8}
unset DISPLAY WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS BROWSER
export TMPDIR="$WORK/tmp"
mkdir -p "$TMPDIR" "$WORK/rt"
export WORK BIN=${BIN:-$(cd "$WORK/.." && pwd)/bin} CLANG=${CLANG:-clang-18} LLVM_LINK=${LLVM_LINK:-llvm-link-18}
export OPTNONE=${OPTNONE:-0} BUDGET=${BUDGET:-100000000} STACK=${STACK:-1048576}
export RT_A="$WORK/rt/rt-A.bc" RT_B="$WORK/rt/rt-B.bc"
"$CLANG" -O0 -Xclang -disable-O0-optnone -gline-tables-only -ffreestanding -emit-llvm -c "$here/rt.c" -o "$RT_A"
"$CLANG" -O0 -Xclang -disable-O0-optnone -gline-tables-only -ffreestanding -DRT_B -emit-llvm -c "$here/rt.c" -o "$RT_B"
find "$corpus" -maxdepth 1 -name '*.c' | sort >"$WORK/tests.txt"
xargs -P "$jobs" -n 1 sh "$here/run-one.sh" <"$WORK/tests.txt" >"$WORK/results.tsv"
