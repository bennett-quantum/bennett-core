#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
: "${SQLITE_DIR:?set SQLITE_DIR to the unmodified SQLite amalgamation directory}"
python3 - "$SQLITE_DIR" <<'CHECK'
import hashlib
from pathlib import Path
import sys
source = Path(sys.argv[1])
for name, expected in {
    'sqlite3.c': '0c657666d819f261cc4007526ad57e1669de9949df7a8ea847aa284a2bf0ee52',
    'sqlite3.h': 'd6c84557261893439b341d2e6c35187ae3ea24fbfd843ad94616f72d89242b8c',
}.items():
    if hashlib.sha256((source/name).read_bytes()).hexdigest() != expected:
        raise SystemExit(f'{name}: expected the unmodified SQLite 3.54.1 release')
CHECK
out=${OUT:-"$root/target/sqlite-on-device/n${NROWS:-4}"}
mkdir -p "$out"
out=$(realpath "$out")
# A wrapper avoids copying or editing the amalgamation; musl link.py accepts one translation unit.
printf '#include "sqlite3.c"\n#include "driver.c"\n' > "$out/program.c"
set -- -DSQLITE_THREADSAFE=0 -DSQLITE_OMIT_LOAD_EXTENSION -DSQLITE_OS_OTHER=1 \
  -DSQLITE_ENABLE_MEMSYS5 -DSQLITE_TEMP_STORE=3 -DNROWS="${NROWS:-4}" \
  -DHEAP_BYTES="${HEAP_BYTES:-262144}" -I"$SQLITE_DIR" -I"$root/experiments/sqlite-on-device"
clang-18 -O2 -fno-vectorize -fno-slp-vectorize "$@" "$out/program.c" -o "$out/native"
# Pass each identical configuration flag to the device compilation.
for flag do
  set -- "$@" "--clang-flag=$flag"
  shift
done
python3 "$root/rt/musl/link.py" "$out/program.c" -o "$out/program.bn" \
  --arena-bytes "${ARENA_BYTES:-4096}" --stack-bytes "${STACK_BYTES:-1048576}" \
  --keep "$out/link" "$@"
