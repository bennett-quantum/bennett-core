# Vendored: GCC 13.3.0 libgcc complex arithmetic

`gcc-13.3.0/` contains **unmodified** files from the official GNU release:
https://gcc.gnu.org/pub/gcc/releases/gcc-13.3.0/gcc-13.3.0.tar.xz (2024-05-21).
Archive SHA-256: `0845e9621c9543a13f484e94584a49ffc0129970e9914624235fc1d061a0c083`.

The detached `.tar.xz.sig` was checked with `gpgv --keyring gnu-keyring.gpg
 gcc-13.3.0.tar.xz.sig gcc-13.3.0.tar.xz`, using the keyring from
https://ftp.gnu.org/gnu/gnu-keyring.gpg. Result: good signature by Jakub Jelinek,
RSA signing key `6C35B99309B5FA62`, fingerprint
`D3A9 3CAD 751C 2AF4 F8C7 AD51 6C35 B993 09B5 FA62`, also published at
https://gcc.gnu.org/mirrors.html. Downloads and verification output are scratch data, not build inputs.

Copied paths: `COPYING`, `COPYING3`, `COPYING.RUNTIME`, `libgcc/libgcc2.c`,
`libgcc/libgcc2.h`, `gcc/tsystem.h`, `include/longlong.h`, `include/filenames.h`,
`include/hashtab.h`, `include/ansidecl.h`. Only libgcc2.c supplies executable code.
The others supply its declarations and macros. All other GCC files are omitted.
`SHA256SUMS` records each copied file; from this directory run `sha256sum -c SHA256SUMS`.

The libgcc source is GPL version 3 (or later) **with the GCC Runtime Library Exception 3.1**,
whose full texts are `COPYING3` and `COPYING.RUNTIME`. Included utility headers carry their own
notices, including GPL version 2 or later (`COPYING`) and, in longlong.h, its additional exception.
Preserve every notice and licence text when updating.

## Why this release is the native reference

Measured on x86-64 Linux, 2026-11-16: `gcc --version` reports Ubuntu GCC
`13.3.0-6ubuntu2~24.04.1`; the installed `libgcc-13-dev` has that same package version.
The C corpus's `run-one.sh` compiles native references with clang 18 at `-O0` and `-O2`,
links `-lm`, and does not override the compiler runtime. Repeating those links with
`-Wl,-Map,native.map` (the exact sweep's runner retains the map) shows **all six** helpers
pulled from GCC 13's **static `libgcc.a`**, members `_mulsc3.o`, `_divsc3.o`, `_muldc3.o`,
`_divdc3.o`, `_mulxc3.o`, `_divxc3.o`. `nm` shows definitions, not dynamic imports;
`ldd` shows no libgcc_s dependency. The installed shared `libgcc-s1` is GCC 14.2, but
it does not supply these helpers to the corpus's native executables.

Distribution patch audit: downloaded
https://archive.ubuntu.com/ubuntu/pool/main/g/gcc-13/gcc-13_13.3.0-6ubuntu2~24.04.1.dsc
and its `gcc-13_13.3.0-6ubuntu2~24.04.1.debian.tar.xz`.
The latter's SHA-256 is
`5523658f272ad6d15a83b6e26d178fbd5cb7709ec7ce2ca52b0c843e19c228e3`, matching the .dsc.
Searching the entire extracted `debian/patches` and packaging rules finds no changes
to `libgcc2.c`, `libgcc2.h` or `libgcc/config/i386`; none of the six helper algorithms
is distribution-patched. The native archive remains the execution oracle, including
compiler optimization choices (especially NaN operand selection).

## Building unchanged source with clang

`front/CMakeLists.txt` compiles **the original libgcc2.c six times**, selecting each
helper with `-DL_mulsc3`, etc., then llvm-links the results into `complex-runtime.bc`.
Flags: `-O2 -ffp-contract=off -ffreestanding -fno-builtin`, no vectorization,
`-emit-llvm -c`. FP operations remain in the bitcode until the front links the helper
into the user program and lowers both over SoftFloat. No GCC build or network is needed.

`config/` is Bennett's separate configuration, **not vendored GCC source**. GCC normally
generates these target build headers; here `tconfig.h` supplies the x86-64 values observed
with `gcc -fbuilding-libgcc -dM -E -x c /dev/null`. SF/DF/XF have 24/53/64 mantissa bits,
no excess precision, the hardware-double switch is on (so `__divsc3` calculates in double),
and min/max/epsilon map to clang's matching predefined constants. HF/TF and all integer
helper bodies are not selected. GCC-only integer declaration modes are mapped to DI;
they contribute no code. The other three configuration headers need no definitions.
The build reads the upstream libgcc2.h, tsystem.h and their dependency headers unchanged.

All six helpers are self-contained: only arithmetic, comparisons, fabs and copysign.
In particular libgcc's **`__divxc3` needs neither logbl nor scalbnl**, removing the old
compiler-rt dependency on musl's constrained ilogbl. Complex-runtime's undefined intrinsic
names can be inspected with `llvm-nm-18 --undefined-only target/front/complex-runtime.bc`.

To update: fetch and verify the next official release, copy the same paths without edits,
update SHA256SUMS and this note, audit target predefines and native linkage again, and run
both the exact sweep and corpus complex-division checks. Never silently change the reference.
