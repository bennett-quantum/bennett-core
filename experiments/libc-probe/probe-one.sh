#!/bin/sh
# Runs one C program natively (system C library) and on the device against a C library compiled to bitcode, and
# prints one tab-separated line: name, outcome, detail, device steps, front seconds, front peak KiB, .bn bytes,
# device functions, device instructions, libc symbols added after a refusal, native output equals .expected,
# device console equals native stdout (CONSOLE=1 only: needs the front of front-effect.patch in BIN).
#
#   probe-one.sh <prog.c>
#
# Environment (set by probe-all.sh): WORK, BIN (bennett-front, bennett), CLANG, LLVM_LINK,
# LIB_MEMBERS (the library's member bitcode files, in archive order), LIB_INDEX (their symbol tables, resolve.py), RT_SRC (the probe runtime, e.g. musl-rt.c), RT_CFLAGS (flags for it),
# PROG_CFLAGS (flags for the program: the library's headers), BUDGET, STACK, FRONT_TIMEOUT (seconds), LINK
# (internalize, the default: archive members, then everything but the entry internal; external: members only).
# DEVICE_EXIT=1 (musl): real exit/console and front-provided init/fini arrays; compare status and console directly.
#
# Oracle: stdout bytes and exit status, as a hash (hash.py, the runtime's hash()). Outcomes: agrees, refused,
# fault (a named fault native did not have), wrong (device output or status differs from native while the run
# reports success), crash, budget, timeout (the front ran out of FRONT_TIMEOUT), skipped (no native reference).
set -u
# Native reference run cap, in seconds.
NATIVE_TIMEOUT_SECONDS=60
src=$1
name=$(basename "$src" .c)
here=$(cd "$(dirname "$0")" && pwd)
out="$WORK/out/$name"
rm -rf "$out"
mkdir -p "$out"
cd "$out" || exit 1
front_s="" front_kib="" bn_bytes="" funcs="" insts="" added="" expected="" console=""

capped() {
  systemd-run --user --scope -q -p MemoryMax=2G -p MemorySwapMax=0 "$@"
}

emit() { # outcome detail [steps]
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$name" "$1" "$2" "${3:-}" "$front_s" "$front_kib" \
    "$bn_bytes" "$funcs" "$insts" "$added" "$expected" "$console" | tee result.tsv
  exit 0
}

# Dialect options from GCC's dg-options, as in experiments/c-corpus/run-one.sh.
flags=""
for opt in $(grep -E 'dg-(additional-)?options' "$src" | grep -v target | tr -d '"{}' |
  sed 's/.*dg-[a-z-]*options//; s/\*\/.*//'); do
  case "$opt" in
  -fgnu89-inline | -fwrapv | -std=gnu89 | -std=c89 | -std=c99 | -fno-strict-overflow | -fno-builtin-abort | \
    -fno-inline | -finput-charset=utf-8) flags="$flags $opt" ;;
  esac
done
common="-w -Wno-error=implicit-function-declaration -Wno-error=implicit-int -Wno-error=int-conversion
  -Wno-error=incompatible-pointer-types -Wno-error=incompatible-function-pointer-types -Wno-error=return-type
  $flags"

# Native reference: -O0 and -O2 must agree on stdout and status.
for level in O0 O2; do
  # shellcheck disable=SC2086
  if ! capped "$CLANG" -$level $common "$src" -o native-$level -lm >native-$level.log 2>&1; then
    emit skipped "clang does not compile it natively"
  fi
  capped timeout "$NATIVE_TIMEOUT_SECONDS" ./native-$level >native-$level.out 2>/dev/null </dev/null
  echo $? >native-$level.status
done
s0=$(cat native-O0.status)
[ "$s0" = "$(cat native-O2.status)" ] || emit skipped "native -O0 and -O2 disagree on status"
cmp -s native-O0.out native-O2.out || emit skipped "native -O0 and -O2 disagree on output"
[ "$s0" = 124 ] && emit skipped "native run timed out"
if [ -f "$src.expected" ]; then
  if cmp -s native-O0.out "$src.expected"; then expected=yes; else expected=no; fi
fi
want=$(python3 "$here/hash.py" native-O0.out "$s0")

# Device: the README's flags against the library's headers.
# shellcheck disable=SC2086
if ! capped "$CLANG" -O0 -Xclang -disable-O0-optnone -gline-tables-only $PROG_CFLAGS $common -emit-llvm -c "$src" \
  -o prog.bc >clang.log 2>&1; then
  emit skipped "clang does not compile it against the library's headers: $(grep -m 1 error clang.log)"
fi
params=$(llvm-dis-18 prog.bc -o - | grep -m 1 '^define .* @main(' | sed 's/.*@main(//; s/).*//')
case "$params" in
"") main_args=0 ;;
*,*,*) main_args=3 ;;
*,*) main_args=2 ;;
*) main_args=2 ;; # main(int): called with (1, argv), as a C runtime does
esac

# Links variant $1 into linked-$1.bc and writes prog-$1.bn; on failure emits the outcome. A refusal of a call
# to a function the library defines (LLVM's optimiser inside the front makes calls such as puts from printf,
# bcmp from memcmp) adds that function as a root and links again, as a linker resolves libcalls after LTO.
build() {
  variant=$1
  rtflag=""
  [ "$variant" = B ] && rtflag=-DRT_B
  [ "${DEVICE_EXIT:-0}" = 1 ] && rtflag="$rtflag -DBENNETT_DEVICE_EXIT"
  [ "${CONSOLE:-0}" = 1 ] && rtflag="$rtflag -DBENNETT_EFFECT"
  # shellcheck disable=SC2086
  if ! "$CLANG" $RT_CFLAGS -DMAIN_ARGS=$main_args -DBENNETT_EXPECT="${want}ull" $rtflag -O0 -Xclang \
    -disable-O0-optnone -gline-tables-only -emit-llvm -c "$RT_SRC" -o rt-$variant.bc >rt-$variant.log 2>&1; then
    emit crash "the probe runtime does not compile: $(grep -m 1 error rt-$variant.log)"
  fi
  roots="" added=""
  for round in 1 2 3 4 5 6 7 8; do
    # As a static linker does with an archive: the members that define what the program and the runtime use,
    # transitively (resolve.py); then every symbol but the entry made internal, so that LLVM's pipeline inside
    # the front drops functions nothing calls (as --gc-sections would). LINK=external keeps them all.
    # shellcheck disable=SC2086
    python3 -I "$here/resolve.py" "$LIB_MEMBERS" "$LIB_INDEX" prog.bc rt-$variant.bc $roots \
      >members-$variant.txt 2>unresolved-$variant.txt
    # shellcheck disable=SC2046
    if ! "$LLVM_LINK" prog.bc rt-$variant.bc $(cat members-$variant.txt) -o linked-$variant.bc \
      >link-$variant.log 2>&1; then
      emit skipped "does not link with the library: $(grep -m 1 . link-$variant.log)"
    fi
    # What a static linker does beyond llvm-link: constructors and destructors into arrays the runtime calls
    # (ctors.py), undefined weak references to null (weak.py, names in weak-*.txt).
    llvm-dis-18 linked-$variant.bc -o linked-$variant.ll || emit crash "llvm-dis failed"
    if [ "${DEVICE_EXIT:-0}" != 1 ]; then
      python3 -I "$here/ctors.py" linked-$variant.ll linked-$variant.ll || emit crash "ctors.py failed"
    fi
    python3 -I "$here/weak.py" linked-$variant.ll linked-$variant.ll 2>weak-$variant.txt &&
      llvm-as-18 linked-$variant.ll -o linked-$variant.bc || emit crash "ctors.py or weak.py failed"
    rm -f linked-$variant.ll
    if [ "${LINK:-internalize}" = internalize ]; then
      keep="__bennett_entry$(echo "$roots" | sed 's/-u /,/g; s/ //g')"
      opt-18 -passes='internalize,globaldce' -internalize-public-api-list="$keep" \
        linked-$variant.bc -o linked-$variant.bc >>link-$variant.log 2>&1
    fi
    capped /usr/bin/time -v -o front-$variant.time timeout "$FRONT_TIMEOUT" "$BIN/bennett-front" \
      linked-$variant.bc -o prog-$variant.bn --region-budget 100000 --stack-bytes "$STACK" >front-$variant.log 2>&1
    status=$?
    front_s=$(sed -n 's/.*Elapsed (wall clock) time (h:mm:ss or m:ss): //p' front-$variant.time |
      awk -F: '{ s = 0; for (i = 1; i <= NF; i++) s = s * 60 + $i; print s }')
    front_kib=$(sed -n 's/.*Maximum resident set size (kbytes): //p' front-$variant.time)
    [ $status = 124 ] && emit timeout "bennett-front ran longer than $FRONT_TIMEOUT s"
    missing=$(sed -n 's/^not handled yet: a call to @\([^,]*\), which has no definition.*/\1/p' front-$variant.log)
    if [ $status = 3 ] && [ -n "$missing" ] && grep -q "	\(strong\|weak\)	$missing\$" "$LIB_INDEX"; then
      roots="$roots -u $missing"
      added="${added:+$added,}$missing"
      continue
    fi
    break
  done
  if [ $status = 3 ] && grep -q '^not handled yet: ' front-$variant.log; then
    emit refused "$(grep -m 1 '^not handled yet: ' front-$variant.log)"
  fi
  [ $status != 0 ] && emit crash "bennett-front exit $status: $(grep -m 1 . front-$variant.log)"
  if ! capped "$BIN/bennett" check prog-$variant.bn >check-$variant.log 2>&1; then
    emit crash "invalid device program: $(grep -m 1 . check-$variant.log)"
  fi
  bn_bytes=$(wc -c <prog-$variant.bn)
  funcs=$(grep -c '^func ' prog-$variant.bn)
  insts=$(grep -cE '^    ' prog-$variant.bn)
}

device_run() {
  capped "$BIN/bennett" run --entry __bennett_entry --budget "$BUDGET" "prog-$variant.bn" >run-$variant.log 2>&1
  status=$?
  steps=$(sed -n 's/^steps: //p' run-$variant.log)
  # CONSOLE=1: the device console (bennett run's `console:` line) against native stdout, escaped the same way.
  if [ "${CONSOLE:-0}" = 1 ]; then
    if [ "$(sed -n 's/^console: //p' run-$variant.log)" = "$(python3 -I "$here/escape.py" native-O0.out)" ]; then
      console=equal
    else
      console=differs
    fi
  fi
  first=$(grep -m 1 -E '^(result|fault|stopped|error|region fault):' run-$variant.log)
  case "$status:$first" in
  0:result:*) kind=result value=$(echo "$first" | sed 's/^result: \([0-9]*\).*/\1/') ;;
  1:fault:*) kind=fault value=$(echo "$first" | sed 's/^fault: \([a-z_]*\).*/\1/') ;;
  1:region* | 1:stopped:* | 2:stopped:*) kind=stopped value=$first ;;
  *) kind=crash value="bennett run exit $status: $(grep -m 1 . run-$variant.log)" ;;
  esac
}

build A
device_run
case "$kind:$value" in
fault:divide_by_zero | fault:bad_indirect)
  first_fault=$value
  build B
  device_run
  case "$first_fault:$kind:$value" in
  bad_indirect:fault:divide_by_zero) emit agrees "exit path, output and status equal native" "$steps" ;;
  divide_by_zero:fault:bad_indirect) emit wrong "exit path: output or status differs from native" "$steps" ;;
  *:fault:"$first_fault") emit fault "$first_fault" "$steps" ;;
  *) emit crash "runtime variants disagree: A $first_fault, B $kind $value" "$steps" ;;
  esac
  ;;
fault:budget) emit budget "$BUDGET" "$steps" ;;
fault:*) emit fault "$value" "$steps" ;;
stopped:*) emit fault "$value" "$steps" ;;
crash:*) emit crash "$value" "$steps" ;;
result:*)
  if [ "${DEVICE_EXIT:-0}" = 1 ]; then
    [ "$((value % 256))" = "$s0" ] && [ "$console" = equal ] &&
      emit agrees "device exit/return status and console equal native" "$steps"
    emit wrong "device status $value, native $s0, console $console" "$steps"
  fi
  [ "$value" = "$want" ] && emit agrees "return path, output and status equal native" "$steps"
  emit wrong "return path: hash $value, native $want (status $s0)" "$steps"
  ;;
esac
emit crash "unclassified: $kind $value" "$steps"
