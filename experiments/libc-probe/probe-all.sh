#!/bin/sh
# Runs probe-one.sh over a list of C programs, in parallel.
#
#   probe-all.sh <file listing .c paths, or a directory> <work dir> [jobs]
#
# LIBDIR: a library built by musl-bc.sh or picolibc-bc.sh (libc.bc, cflags, sysroot/include, rt.c). Binaries from
# DEVICE_EXIT=1 selects musl's real device exit/console mode and skips ctors.py.
# BIN (default <work dir>/../bin). BUDGET (default 10^9 steps), STACK (1 MiB), FRONT_TIMEOUT (900 s), LINK.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
WORK=$(mkdir -p "$2" && cd "$2" && pwd)
jobs=${3:-32}
: "${LIBDIR:?set LIBDIR to a library built by musl-bc.sh or picolibc-bc.sh}"
LIBDIR=$(cd "$LIBDIR" && pwd)
BIN=$(cd "${BIN:-$WORK/../bin}" && pwd)
unset DISPLAY WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS BROWSER
export TMPDIR="$WORK/tmp"
mkdir -p "$TMPDIR"
# The members' symbol index, written once (resolve.py with no inputs).
[ -f "$LIBDIR/index.tsv" ] || python3 -I "$here/resolve.py" "$LIBDIR/members.txt" "$LIBDIR/index.tsv"
export WORK BIN CLANG=${CLANG:-clang-18} LLVM_LINK=${LLVM_LINK:-llvm-link-18}
export BUDGET=${BUDGET:-1000000000} STACK=${STACK:-1048576}
if [ "${DEVICE_EXIT:-0}" = 1 ]; then export CONSOLE=1; fi
export FRONT_TIMEOUT=${FRONT_TIMEOUT:-900} LINK=${LINK:-internalize}
export LIB_MEMBERS="$LIBDIR/members.txt" LIB_INDEX="$LIBDIR/index.tsv" RT_SRC=${RT_SRC:-$LIBDIR/rt.c} RT_CFLAGS PROG_CFLAGS
RT_CFLAGS=$(cat "$LIBDIR/cflags")
PROG_CFLAGS=${PROG_CFLAGS:-$(cat "$LIBDIR/prog-cflags")}
if [ -d "$1" ]; then
  find "$(cd "$1" && pwd)" -maxdepth 1 -name '*.c' | sort >"$WORK/tests.txt"
else
  cp "$1" "$WORK/tests.txt"
fi
xargs -P "$jobs" -n 1 sh "$here/probe-one.sh" <"$WORK/tests.txt" >"$WORK/results.tsv"
