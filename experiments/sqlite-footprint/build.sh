#!/usr/bin/env bash
# Build the traced and the plain (perf) driver.
#   SQLITE_DIR=<dir with sqlite3.c and sqlite3.h> OUT=<build dir> ./build.sh
set -euo pipefail
: "${SQLITE_DIR:?set SQLITE_DIR to the directory holding the SQLite amalgamation}"
: "${OUT:?set OUT to a build directory}"
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$OUT"
DEFS="-DSQLITE_THREADSAFE=0 -DSQLITE_ENABLE_MEMSYS5 -DSQLITE_OMIT_LOAD_EXTENSION"

# Traced SQLite: instrumented IR -> rewrite -> codegen only (no second optimisation).
clang -O2 -gline-tables-only -S -emit-llvm $DEFS \
  -fsanitize-coverage=func,trace-pc-guard,trace-loads,trace-stores \
  -finstrument-functions-after-inlining \
  "$SQLITE_DIR/sqlite3.c" -o "$OUT/sqlite3.cov.ll"
python3 "$HERE/rewrite_ir.py" "$OUT/sqlite3.cov.ll" "$OUT/sqlite3.tr.ll" "$OUT/sites.tsv" "$OUT/blocks.tsv"
clang -O2 -c -Xclang -disable-llvm-passes "$OUT/sqlite3.tr.ll" -o "$OUT/sqlite3.tr.o"
clang -O2 -g -c "$HERE/hooks.c" -o "$OUT/hooks.o"
clang -O2 -g -DTRACE -I"$SQLITE_DIR" -c "$HERE/driver.c" -o "$OUT/driver.tr.o"
clang -O2 "$OUT/driver.tr.o" "$OUT/sqlite3.tr.o" "$OUT/hooks.o" -lm -o "$OUT/driver_trace"

# Plain SQLite for the x86 instruction-count cross-check.
clang -O2 $DEFS -c "$SQLITE_DIR/sqlite3.c" -o "$OUT/sqlite3.plain.o"
clang -O2 -DPERF -I"$SQLITE_DIR" -c "$HERE/driver.c" -o "$OUT/driver.perf.o"
clang -O2 "$OUT/driver.perf.o" "$OUT/sqlite3.plain.o" -lm -o "$OUT/driver_perf"



echo built
