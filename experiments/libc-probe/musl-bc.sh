#!/bin/sh
# Compiles every C source of musl's libc (x86_64, the sources musl's own Makefile selects) to LLVM bitcode with
# the README's clang flags, and writes one line per source: path, status (ok | fails | asm-source).
#
#   musl-bc.sh <out dir> [overlay dir ...]
#
# MUSL_TARBALL: the musl 1.2.5 source archive (extracted into <out dir>/src on first use). The optional overlay
# directories are searched, in order, before musl's arch headers, so a file there (syscall_arch.h, atomic_arch.h,
# pthread_arch.h) replaces musl's own; GENERIC=1 also replaces musl's x86_64 assembly or C files by the generic
# C files of the same name where musl has one (memcpy, memmove, memset, the libm functions). MUSL_PATCHES:
# patch files (absolute paths, -p1) applied to the extracted sources, e.g. musl-printf-nofp.patch. JOBS (default 32).
# Output: <out dir>/bc/<path>.bc, <out dir>/compile.tsv, <out dir>/libc.bc (all bitcode linked into one module,
# with llvm-link, a library module for --only-needed), <out dir>/libc.a (llvm-ar archive of the members),
# <out dir>/sysroot/include (musl's public headers, for programs), <out dir>/cflags (the flags musl itself used).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
out=$(mkdir -p "$1" && cd "$1" && pwd)
shift
inc=""
for dir in "$@"; do
  inc="$inc -I$(cd "$dir" && pwd)"
done
CLANG=${CLANG:-clang-18}
LLVM_LINK=${LLVM_LINK:-llvm-link-18}
JOBS=${JOBS:-32}
: "${MUSL_TARBALL:?set MUSL_TARBALL to the musl 1.2.5 archive}"

src="$out/src/musl-1.2.5"
if [ ! -f "$src/obj/include/bits/syscall.h" ]; then
  mkdir -p "$out/src"
  tar -xzf "$MUSL_TARBALL" -C "$out/src"
  for patch in ${MUSL_PATCHES:-}; do
    patch -s -d "$src" -p1 <"$patch"
  done
  (cd "$src" && CC=$CLANG ./configure --target=x86_64 --disable-shared >configure.log 2>&1 &&
    make -s obj/include/bits/alltypes.h obj/include/bits/syscall.h obj/src/internal/version.h &&
    make -s install-headers DESTDIR="$out/sysroot" prefix=/ >/dev/null)
fi

# The object list exactly as musl's Makefile computes it (arch files replace generic ones of the same name).
printf 'include Makefile\nprint-objs:\n\t@echo $(filter-out $(REPLACED_OBJS), $(sort $(BASE_OBJS) $(ARCH_OBJS)))\n' \
  >"$out/print.mk"
(cd "$src" && make -s -f "$out/print.mk" print-objs) | tr ' ' '\n' | grep '^src/' >"$out/objs.txt"
: >"$out/sources.txt"
while read -r obj; do
  base=${obj%.o}
  if [ "${GENERIC:-0}" = 1 ]; then
    generic=$(echo "$base" | sed 's|/x86_64/|/|')
    if [ "$generic" != "$base" ] && [ -f "$src/$generic.c" ]; then
      echo "$generic.c" >>"$out/sources.txt"
      continue
    fi
  fi
  for ext in c s S; do
    if [ -f "$src/$base.$ext" ]; then
      echo "$base.$ext" >>"$out/sources.txt"
    fi
  done
done <"$out/objs.txt"

echo "-std=c99 -nostdinc -ffreestanding -fexcess-precision=standard -frounding-math -fno-strict-aliasing \
-fno-stack-protector -w -Qunused-arguments -D_XOPEN_SOURCE=700 $inc -I$src/arch/x86_64 -I$src/arch/generic \
-I$src/obj/src/internal -I$src/src/include -I$src/src/internal -I$src/obj/include -I$src/include" >"$out/cflags"
cat >"$out/cc.sh" <<EOF
#!/bin/sh
# One source: compile to bitcode, print one line.
f=\$1
case "\$f" in
*.s | *.S) printf '%s\tasm-source\t\n' "\$f"; exit 0 ;;
esac
mkdir -p "$out/bc/\$(dirname "\$f")"
cd "$src" || exit 1
if $CLANG -std=c99 -nostdinc -ffreestanding -fexcess-precision=standard -frounding-math -fno-strict-aliasing \\
  -fno-stack-protector -w -Qunused-arguments -D_XOPEN_SOURCE=700 $inc -I./arch/x86_64 -I./arch/generic \\
  -Iobj/src/internal -I./src/include -I./src/internal -Iobj/include -I./include \\
  -O0 -Xclang -disable-O0-optnone -gline-tables-only -emit-llvm -c "\$f" -o "$out/bc/\$f.bc" \\
  >"$out/bc/\$f.log" 2>&1; then
  printf '%s\tok\t\n' "\$f"
else
  printf '%s\tfails\t%s\n' "\$f" "\$(grep -m 1 'error' "$out/bc/\$f.log")"
fi
EOF
rm -rf "$out/bc"
xargs -P "$JOBS" -n 1 sh "$out/cc.sh" <"$out/sources.txt" | sort >"$out/compile.tsv"

# One library module, and an archive of the members.
grep '	ok	' "$out/compile.tsv" | cut -f 1 | sed "s|^|$out/bc/|; s|\$|.bc|" >"$out/members.txt"
rm -f "$out/libc.a"
# shellcheck disable=SC2046
llvm-ar-18 rcs "$out/libc.a" $(cat "$out/members.txt")
# shellcheck disable=SC2046
"$LLVM_LINK" $(cat "$out/members.txt") -o "$out/libc.bc" >"$out/link.log" 2>&1 || echo "llvm-link of the library failed: see $out/link.log"
echo "-nostdinc -isystem $out/sysroot/include" >"$out/prog-cflags"
cp "$here/musl-rt.c" "$out/rt.c"
python3 "$here/classify.py" "$out" >"$out/classify.tsv"
