#!/bin/sh
# Runs one self-checking C test through a native build and through clang -> bennett-front -> bennett run, and
# prints one tab-separated line: name, outcome, detail, device steps.
#
#   run-one.sh <test.c>
#
# Environment (set by run-all.sh): WORK (scratch root), BIN (bennett-front and bennett), RT_A, RT_B (runtime
# bitcode, variants A and B, see rt.c), BUDGET (device step budget), STACK (device stack bytes), CLANG,
# LLVM_LINK, OPTNONE (1: keep clang's optnone on the test's functions). Every stage runs under a 2 GiB memory cap with no swap. The device run is bounded by BUDGET;
# bennett-front and clang have no step budget and are bounded only by the memory cap.
set -u
src=$1
name=$(basename "$src" .c)
out="$WORK/out/$name"
rm -rf "$out"
mkdir -p "$out"
cd "$out" || exit 1

capped() {
  systemd-run --user --scope -q -p MemoryMax=2G -p MemorySwapMax=0 "$@"
}

emit() { # outcome detail [steps]
  printf '%s\t%s\t%s\t%s\n' "$name" "$1" "$2" "${3:-}" | tee result.tsv
  exit 0
}

# Options from the test's dg-options and dg-additional-options (quoted or in braces; lines restricted to a
# target are ignored) that clang understands and that change meaning; the rest are GCC tuning.
flags=""
for opt in $(grep -E 'dg-(additional-)?options' "$src" | grep -v target | tr -d '"{}' |
  sed 's/.*dg-[a-z-]*options//; s/\*\/.*//'); do
  case "$opt" in
  -fgnu89-inline | -fwrapv | -std=gnu89 | -std=c89 | -std=c99 | -fno-strict-overflow | -fno-builtin-abort | \
    -fno-inline | -finput-charset=utf-8) flags="$flags $opt" ;;
  esac
done
# Old-style C is ordinary in this corpus; clang 18 makes these errors by default.
common="-w -Wno-error=implicit-function-declaration -Wno-error=implicit-int -Wno-error=int-conversion
  -Wno-error=incompatible-pointer-types -Wno-error=incompatible-function-pointer-types -Wno-error=return-type
  $flags"

# Native reference: -O0 and -O2 must agree, and the test must pass (exit status 0).
for level in O0 O2; do
  # shellcheck disable=SC2086
  if ! capped "$CLANG" -$level $common "$src" -o native-$level -lm >native-$level.log 2>&1; then
    emit skipped "clang does not compile it natively"
  fi
  capped ./native-$level >/dev/null 2>&1 </dev/null
  echo $? >native-$level.status
done
s0=$(cat native-O0.status)
s2=$(cat native-O2.status)
[ "$s0" = "$s2" ] || emit skipped "native -O0 and -O2 disagree ($s0 vs $s2)"
[ "$s0" = 0 ] || emit skipped "native run does not pass (status $s0)"

# Device: the README's flags, plus the same dialect flags. With OPTNONE=1 the bitcode keeps clang's `optnone`,
# so LLVM's pipeline inside bennett-front leaves the test's functions as clang -O0 wrote them (a stress mode:
# less is folded away before bennett-front lowers it).
optnone="-Xclang -disable-O0-optnone"
[ "${OPTNONE:-0}" = 1 ] && optnone=""
# shellcheck disable=SC2086
if ! capped "$CLANG" -O0 $optnone -gline-tables-only $common -emit-llvm -c "$src" -o prog.bc \
  >clang.log 2>&1; then
  emit skipped "clang does not emit bitcode for it"
fi

# Builds variant $1 (A or B) into prog-$1.bn; on failure emits the outcome.
build() {
  rt=$RT_A
  [ "$1" = B ] && rt=$RT_B
  if ! "$LLVM_LINK" "$rt" --override prog.bc -o linked-$1.bc >link-$1.log 2>&1; then
    emit skipped "the runtime does not link with it: $(head -n 1 link-$1.log)"
  fi
  capped "$BIN/bennett-front" linked-$1.bc -o prog-$1.bn --region-budget 100000 --stack-bytes "$STACK" \
    >front-$1.log 2>&1
  status=$?
  if [ $status = 3 ] && grep -q '^not handled yet: ' front-$1.log; then
    emit refused "$(grep -m 1 '^not handled yet: ' front-$1.log)"
  fi
  if [ $status != 0 ]; then
    emit crash "bennett-front exit $status: $(grep -m 1 . front-$1.log)"
  fi
  if ! capped "$BIN/bennett" check prog-$1.bn >check-$1.log 2>&1; then
    emit crash "invalid device program: $(grep -m 1 . check-$1.log)"
  fi
}

# Runs prog-$1.bn; sets kind (result|fault|stopped|crash), value and steps.
device_run() {
  # A main with parameters gets argc 1 and a null argv.
  sig=$(grep -m 1 '^func @main(' "prog-$1.bn" 2>/dev/null)
  set --
  case "$sig" in
  *"func @main()"*) ;;
  *,*) set -- --arg 1 --arg 0 ;;
  *) set -- --arg 1 ;;
  esac
  capped "$BIN/bennett" run --budget "$BUDGET" "$@" "prog-$variant.bn" >run-$variant.log 2>&1
  status=$?
  steps=$(sed -n 's/^steps: //p' run-$variant.log)
  first=$(grep -m 1 -E '^(result|fault|stopped|error):' run-$variant.log)
  case "$status:$first" in
  0:result:*) kind=result value=$(echo "$first" | sed 's/^result: \([0-9a-z]*\).*/\1/') ;;
  1:fault:*) kind=fault value=$(echo "$first" | sed 's/^fault: \([a-z_]*\).*/\1/') ;;
  1:stopped:* | 2:stopped:*) kind=stopped value=${first#stopped: } ;;
  *) kind=crash value="bennett run exit $status: $(grep -m 1 . run-$variant.log)" ;;
  esac
}

variant=A
build A
device_run A
case "$kind:$value" in
fault:divide_by_zero | fault:bad_indirect)
  first_fault=$value
  variant=B
  build B
  device_run B
  case "$first_fault:$kind:$value" in
  divide_by_zero:fault:bad_indirect) emit wrong "abort() or exit(non-zero) reached" "$steps" ;;
  bad_indirect:fault:divide_by_zero) emit agrees "exit(0)" "$steps" ;;
  *:fault:"$first_fault") emit fault "$first_fault" "$steps" ;;
  *) emit crash "runtime variants disagree: A $first_fault, B $kind $value" "$steps" ;;
  esac
  ;;
fault:budget) emit budget "$BUDGET" "$steps" ;;
fault:*) emit fault "$value" "$steps" ;;
stopped:*) emit fault "stopped: $value" "$steps" ;;
crash:*) emit crash "$value" "$steps" ;;
result:none) emit agrees "main returned no value" "$steps" ;;
result:*)
  if [ $((value % 256)) = 0 ]; then
    emit agrees "main returned $value" "$steps"
  fi
  emit wrong "main returned $value, native status 0" "$steps"
  ;;
esac
emit crash "unclassified: $kind $value" "$steps"
