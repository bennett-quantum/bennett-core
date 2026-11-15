#!/bin/sh
# Granularity of a refusal: each program here through the README pipeline with no C library; one line each:
# name, front exit status, first line of its output (refusal or nothing), device result.
#
#   run.sh <bin dir> <work dir>
set -eu
here=$(cd "$(dirname "$0")" && pwd)
bin=$(cd "$1" && pwd)
work=$(mkdir -p "$2" && cd "$2" && pwd)
for src in "$here"/*.c; do
  name=$(basename "$src" .c)
  clang-18 -O0 -Xclang -disable-O0-optnone -gline-tables-only -emit-llvm -c "$src" -o "$work/$name.bc"
  status=0
  "$bin/bennett-front" "$work/$name.bc" -o "$work/$name.bn" --region-budget 100000 --stack-bytes 65536 \
    >"$work/$name.log" 2>&1 || status=$?
  result=""
  [ $status = 0 ] && result=$("$bin/bennett" run "$work/$name.bn" 2>&1 | grep -m 1 '^result:\|^fault:' || true)
  printf '%s\t%s\t%s\t%s\n' "$name" "$status" "$(head -n 1 "$work/$name.log" | sed 's/, at .*//')" "$result"
done
