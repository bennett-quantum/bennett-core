#!/bin/sh
# Compiles picolibc's C library sources (x86_64, its own headers, the configuration below) to LLVM bitcode with the
# README's clang flags, and writes one line per source: path, status (ok | fails | asm-source), detail.
#
#   picolibc-bc.sh <out dir>
#
# PICOLIBC_DIR: a picolibc checkout (git clone --branch <tag>). MESON: the meson binary (default meson; it needs
# ninja on PATH). PICOLIBC_OPTIONS: extra meson -D options. PICOLIBC_PATCHES: patch files applied to a copy of
# the checkout. JOBS (default 32).
# meson only configures (it writes picolibc.h and the compile database); picolibc-cmds.py turns the database into
# bitcode compiles. Output as musl-bc.sh's: <out dir>/bc, compile.tsv, members.txt, cflags (for the probe runtime),
# prog-cflags (for programs), rt.c (picolibc-rt.c), classify.tsv.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=$(mkdir -p "$1" && cd "$1" && pwd)
: "${PICOLIBC_DIR:?set PICOLIBC_DIR to a picolibc checkout}"
# A copy of the checkout, so that PICOLIBC_PATCHES (patch files, absolute paths, -p1) apply to it alone.
if [ ! -f "$out/src/picolibc/meson.build" ]; then
  mkdir -p "$out/src/picolibc"
  (cd "$PICOLIBC_DIR" && tar --exclude=.git -cf - .) | tar -xf - -C "$out/src/picolibc"
  for patch in ${PICOLIBC_PATCHES:-}; do
    patch -s -d "$out/src/picolibc" -p1 <"$patch"
  done
fi
src="$out/src/picolibc"
MESON=${MESON:-meson}
JOBS=${JOBS:-32}
cat >"$out/cross.txt" <<CROSS
[binaries]
c = ['clang-18', '-target', 'x86_64-pc-linux-gnu', '-ffreestanding', '-fno-pic', '-fno-PIE', '-nostdlib']
c_ld = 'bfd'
ar = 'llvm-ar-18'
nm = 'llvm-nm-18'
strip = 'llvm-strip-18'

[host_machine]
system = 'none'
cpu_family = 'x86_64'
cpu = 'x86_64'
endian = 'little'

[properties]
skip_sanity_check = true
CROSS
# Integer printf and scanf with long long (tinystdio's "long-long" variant), stdin/stdout/stderr over POSIX
# read and write (posix-console), stdio flushed at exit, one thread, no thread-local storage, no semihosting.
# shellcheck disable=SC2086
[ -f "$out/build/compile_commands.json" ] || (cd "$src" && "$MESON" setup "$out/build" --cross-file "$out/cross.txt" \
  -Dmultilib=false -Dtests=false -Dpicocrt=false -Dsemihost=false -Dformat-default=long-long -Dio-long-long=true \
  -Dsingle-thread=true -Dthread-local-storage=false -Dstdio-exit-flush=true -Dposix-console=true \
  -Dfortify-source=none -Dspecsdir=none -Dlibdl=false ${PICOLIBC_OPTIONS:-} >"$out/setup.log" 2>&1)
rm -rf "$out/bc"
python3 -I "$here/picolibc-cmds.py" "$out/build" "$out/bc" >"$out/cmds.tsv"
python3 -I "$here/picolibc-cmds.py" "$out/build" "$out/bc" --cflags >"$out/cflags"
cat >"$out/cc.sh" <<'CC'
#!/bin/sh
# One line of cmds.tsv: compile, print path, status, detail.
line=$1
rel=$(printf '%s' "$line" | cut -f 1)
kind=$(printf '%s' "$line" | cut -f 2)
cmd=$(printf '%s' "$line" | cut -f 3)
if [ "$kind" = asm-source ]; then
  printf '%s\tasm-source\t\n' "$rel"
  exit 0
fi
detail=""
[ "$kind" = generic ] && detail="generic C for an x86 assembly source"
if log=$(sh -c "$cmd" 2>&1); then
  printf '%s\tok\t%s\n' "$rel" "$detail"
else
  printf '%s\tfails\t%s\n' "$rel" "$(printf '%s' "$log" | grep -m 1 error)"
fi
CC
tr '\n' '\0' <"$out/cmds.tsv" | xargs -0 -P "$JOBS" -n 1 sh "$out/cc.sh" | sort >"$out/compile.tsv"
grep '	ok	' "$out/compile.tsv" | cut -f 1 | sed "s|^|$out/bc/|; s|\$|.bc|" >"$out/members.txt"
echo "-nostdlibinc -isystem $out/build -isystem $out/build/libc/include -isystem $src/libc/include" >"$out/prog-cflags"
cp "$here/picolibc-rt.c" "$out/rt.c"
python3 "$here/classify.py" "$out" >"$out/classify.tsv"
