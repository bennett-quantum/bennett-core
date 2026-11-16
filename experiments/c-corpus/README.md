# C corpus: does the C front end handle ordinary C correctly, or refuse it by name?

The question: three testers found silent miscompilations of ordinary C that the 15-program hand corpus missed. The
design rule is that `bennett-front` either handles a program correctly, refuses it by name, or the device program ends
in a named fault; never a silent wrong result, a crash or a hang. How true is that on a few thousand real self-checking
test programs, and why are programs refused?

Measured twice on 2026-11-13, a third time on 2026-11-15 after floating point, a fourth time on 2026-11-15
with the C library linked (musl 1.2.5, `rt/musl`), and a fifth time with the library linked at `b961292`, after
the 128-bit conversions and the complex helpers, and a sixth at `8bbb56c`, which returns a small aggregate as one
integer; the newest section comes first. After that only the four programs that waited on `setjmp` and
`longjmp` were run again, alone.
First at commit `4b65247` (tree clean when the binaries were copied): `bennett-front` and `bennett` were built
once (`cargo build --release`, `front/test.sh`, all tests passing), then copied to `.tmp/c-corpus/bin/`, and
only those copies were used. File and line references in the sections on the
first measurement are at `4b65247`. Then again for step 5, which widened the C the front handles, with the same
harness: at `b5ffb85` before the step, and at `b5ffb85` plus the step's changes (not yet committed; binaries
copied to `.tmp/c-corpus/s5-r5/bin/`). Its results follow the floating-point section.

## Method

**Corpora.**

- GCC 14.2.0 `gcc/testsuite/gcc.c-torture/execute`: the 1,656 top-level `*.c` files (the `ieee/` and `builtins/`
  subdirectories were not run). Each test calls `abort()` on a wrong result and exits 0 on success. GPL; kept under
  `.tmp/c-corpus/src/` only. Fetched with
  `git clone --depth 1 --branch releases/gcc-14.2.0 --filter=blob:none --sparse https://gcc.gnu.org/git/gcc.git gcc`
  then `git sparse-checkout set gcc/testsuite/gcc.c-torture/execute` (commit `04696df0`).
- c-testsuite (`https://github.com/c-testsuite/c-testsuite`, commit `5c727565`, `tests/single-exec`, 220 programs),
  reported separately. Its programs return 0 on success; their printed output is not compared.
- LLVM's `SingleSource/UnitTests` was not run.

**Per program** (`run-one.sh`, fanned out 8 wide by `run-all.sh` with `xargs -P`):

1. Native reference: `clang-18 -O0` and `clang-18 -O2`, each linked against the system C library and run. If clang
   cannot compile the program, the two levels disagree, or the program does not exit 0, it is **skipped** with that
   reason. Both builds get `-w`, downgrades for errors clang 18 makes of old-style C (implicit declarations, implicit
   int, int conversions, incompatible pointers), and the meaning-changing options from the test's own `dg-options` and
   `dg-additional-options` (`-std=gnu89`, `-fwrapv`, `-fgnu89-inline`, `-fno-strict-overflow`, ...). Other GCC
   options are ignored.
2. Device: the README's command (`clang -O0 -Xclang -disable-O0-optnone -gline-tables-only -emit-llvm`) with the
   same dialect options; `llvm-link-18 rt.bc --override prog.bc` (the whole runtime, the test's own definitions
   win); `bennett-front --region-budget 100000 --stack-bytes 1048576`; `bennett check`; then
   `bennett run --budget 100000000`. A `main` with parameters gets `argc = 1, argv = 0` (38 GCC tests).
3. Every stage runs in `systemd-run --user --scope -p MemoryMax=2G -p MemorySwapMax=0`. The device run is bounded by
   its step budget (10^8 steps, about 1.7 s at the measured 6·10^7 steps/s). `bennett-front`, clang and the native
   binary have no step budget; they are bounded only by the memory cap. One native binary did hang, because of a
   harness bug that was then fixed (below). No time limits appear in the scripts.

**Runtime** (`rt.c`, written for this experiment, compiled with `-ffreestanding`). The device has no C library and
only two ways to end: return from `main`, or a named fault. `bennett-front` refuses calls to undefined functions, and
marker calls cannot be used for this because any function that calls a marker, directly or transitively, becomes a
control function. So:

| C | device, variant A | variant B |
|---|---|---|
| `abort()`, `exit(n != 0)` | fault `divide_by_zero` | fault `bad_indirect` |
| `exit(0)` | fault `bad_indirect` | fault `divide_by_zero` |
| return from `main` | `result: r`, compared as `r mod 256` | same |

Each run uses variant A. If it ends in `divide_by_zero` or `bad_indirect`, the program is rebuilt with variant B and run
again: if the fault kind swaps, the runtime raised it; if it stays the same, the program itself faulted. The faults
are forced through volatile globals so that LLVM cannot fold them away; if one did not fault, a loop would run into
the step budget instead. `memcpy memmove memset memcmp bcmp memchr strlen strcmp strncmp strcpy stpcpy strncpy
strcat strchr strrchr atoi abs labs llabs` are plain loops. `malloc`/`calloc`/`realloc` use a bump allocator over
1 MiB, and `free` does nothing. `printf` and the other stdio functions are deliberately left undefined, so they are
refused by name. Limits: `calloc` has no overflow check (see the refusal note below), and `realloc` copies up to n
bytes, not the old block's size. A harness self-test (`.tmp/c-corpus/selftest`) confirmed the classifications for
`wrong`, `exit(0)`, return 0 and `budget`.

**Classes.** **agrees**: the device ends in `exit(0)` or `main` returns a value ≡ 0 (mod 256). **refused**:
`bennett-front` exits 3 with `not handled yet: …`. **fault**: a named fault the program itself raised, or a named run
error. **wrong**: the device reaches `abort()`/`exit(≠0)`, or `main` returns a value ≢ 0. **crash**: `bennett-front`
exits other than 0 or 3, `bennett check` rejects its output, or `bennett run` exits other than 0, 1 or 2. **budget**:
the 10^8-step budget ran out. **skipped**: no native reference, as above.

## The program that waited on over-aligned variadic arguments (2026-11-16)

After variadic arguments aligned to more than 16 bytes were placed where clang's `va_arg` reads them
(`docs/DESIGN.md`, "Variadic functions"), only GCC `pr92904` was run again, with `run-one.sh` (binaries built
from the job's tree and copied), with musl linked (`MUSL_LIBDIR`) and without the library (`rt.c`):

| program | before (`8bbb56c`) | with musl | without the library |
|---|---|---|---|
| `pr92904` | refused: a variadic argument aligned to more than 16 bytes | agrees (67,967 steps) | agrees (65,918 steps) |

The same with `OPTNONE=1`: agrees (without the library in 28,385 steps). Natively it passes at `-O0` and `-O2`.

## The programs that waited on the return address (2026-11-15)

After `llvm.returnaddress` became the device's `retaddr` (`docs/DESIGN.md`, "Return addresses"), only the
three GCC programs refused on it were run again, with `run-one.sh` one at a time (binaries built from the job's
tree and copied), with musl linked (`MUSL_LIBDIR`) and without the library (`rt.c`):

| program | before | with musl | without the library |
|---|---|---|---|
| `20030323-1` | refused: `llvm.returnaddress` | agrees (2,048 steps) | agrees (1 step) |
| `20030811-1` | refused: `llvm.returnaddress` | agrees (2,048 steps) | agrees (1 step) |
| `pr17377` | refused: `llvm.returnaddress` | agrees (2,149 steps) | agrees, `exit(0)` (108 steps) |

The same with `OPTNONE=1`: all three agree (`pr17377` without the library in 13 steps: the front's optimiser
may inline a function clang -O0 marked `noinline` only because of `optnone`). The first two only need the
return address to compile (their functions are never called); `pr17377` checks that one site gives one address
across five calls. With nothing else changed, GCC with musl would be 1,567 agree and 2 refused (inline assembly
1, an over-aligned variadic argument 1).

## The programs that waited on `setjmp` and `longjmp` (2026-11-15)

After `setjmp` and `longjmp` became device instructions (`docs/DESIGN.md`, "Non-local jumps"), only the four
GCC programs refused on them were run again, with `run-one.sh` one at a time (binaries built from the job's
tree and copied), with musl linked (`MUSL_LIBDIR`) and without the library (`rt.c`):

| program | before (`8bbb56c`) | with musl | without the library |
|---|---|---|---|
| `20210505-1` | refused: `longjmp` has no definition | agrees (2,059 steps) | agrees (10 steps) |
| `pr56982` | refused: `_setjmp` (with musl: `setjmp`) has no definition | agrees (2,074 steps) | agrees, `exit(0)` (33 steps) |
| `built-in-setjmp` | refused: `llvm.eh.sjlj.longjmp` | agrees (2,161 steps) | agrees, `exit(0)` (130 steps) |
| `pr84521` | refused: `llvm.eh.sjlj.longjmp` | agrees (2,122 steps) | agrees (73 steps) |

The same with `OPTNONE=1` (the test's functions as clang -O0 wrote them): all four agree. With nothing else
changed, GCC with musl would be 1,564 agree and 5 refused (`llvm.returnaddress` 3, inline assembly 1, an
over-aligned variadic argument 1).

## The three corpora at `8bbb56c`, with the C library linked (2026-11-15)

Same method as the next section, at `8bbb56c`: `6418e6b` (fresh names and debug locations without rescanning) and
`8bbb56c` (an aggregate of at most 128 bits of integer and float fields is returned as one integer instead of
through memory; the always-inline of runtime routines that return an aggregate, `a9ce585`, removed) on top of
`b961292`. Wall time: GCC 267 s, c-testsuite 36 s, `ieee/` 13 s. Compared per program with `b961292`:

| outcome | GCC | c-testsuite | GCC `ieee/` |
|---|---:|---:|---:|
| agrees | **1,560** | **217** | **64** |
| refused by name | 9 | 1 | 3 |
| named fault (native passed) | 6 | 1 | 0 |
| **wrong** | **0** | **0** | **0** |
| crash | 0 | 0 | 0 |
| budget | 0 | 1 | 0 |
| skipped | 81 | 0 | 1 |

No program changed outcome or detail (the three `ieee/` refusals differ only in the exported tree's path), so the
per-reason table of the next section stands. Forty programs that agree both times changed step count:

- 25 GCC programs got cheaper, by 1 to 128 steps; the largest are `960512-1` (2,176 to 2,048), `pr85582-3`
  (2,275 to 2,149), `20030613-1` (2,101 to 2,048), `pr85582-2` (2,131 to 2,089).
- 15 got dearer, all of them programs using `long double` or formatting a floating-point value through musl's
  `printf`, plus `20070614-1` (`__mulsc3`, 3,662 to 3,672): GCC `960215-1` (3,673 to 4,052), `regstack-1`
  (3,954 to 4,354), `960513-1` (2,676 to 2,776), `20030914-1` (4,427 to 4,555), `struct-ret-1` (28,992 to
  29,610), `930513-1`, `920501-8`, `stdarg-2`; c-testsuite `00204` (1,062,821 to 1,089,594), `00174` (49,778 to
  50,744), `00195` (7,563 to 7,769), `00175` (23,518 to 23,638); `ieee/` `920810-1` (+9), `20010226-1` (+8).
  Several are now above their count before `a9ce585` (the run before the next section): `960215-1` 3,863,
  `regstack-1` 4,145, `struct-ret-1` 29,112, `00204` 1,068,923, `00174` 50,015. `20020413-1` is back at its
  count before `a9ce585` (3,298).

## The three corpora at `b961292`, with the C library linked (2026-11-15)

The method of "Results with the C library linked" below: the tree at `b961292` exported with `git archive`,
`bennett-front` with its float and complex runtimes and the release `bennett` copied from builds of the same
sources, the library built from the exported tree, 6 programs in parallel, `ieee/` with `EXTRA_FLAGS=-fno-inline`.
Wall time: GCC 372 s, c-testsuite 30 s, `ieee/` 10 s. Compared per program with the previous run: `72807c5`
plus the init/fini bounds change (`713d996`), before the front inlined runtime routines that return an
aggregate (`a9ce585`), the 128-bit conversions and the complex helpers (`cd2d920`):

| outcome | GCC | c-testsuite | GCC `ieee/` |
|---|---:|---:|---:|
| agrees | **1,560** (was 1,557) | **217** | **64** |
| refused by name | 9 (was 12) | 1 | 3 |
| named fault (native passed) | 6 | 1 | 0 |
| **wrong** | **0** | **0** | **0** |
| crash | 0 | 0 | 0 |
| budget | 0 | 1 | 0 |
| skipped | 81 | 0 | 1 |

Three programs changed outcome, all from refused to agrees: GCC `20070614-1` (`__mulsc3`, 3,662 steps) and
`complex-5` (`__divsc3`, 3,263 steps) through the complex helpers, and `pr49218` (`float` to `i128`, 2,233
steps) through the 128-bit conversions. Every other program has the same outcome and detail. Fifteen that agree
both times changed step count, all programs using `long double` or formatting a floating-point value through
musl's `printf` (which computes in `long double`): GCC `20020413-1`, `20030914-1`, `920501-8`, `930513-1`,
`960215-1`, `960513-1`, `regstack-1`, `stdarg-2`, `struct-ret-1`; c-testsuite `00174`, `00175`, `00195`,
`00204`; `ieee/` `20010226-1`, `920810-1` (largest: `00204` 1,068,923 to 1,062,821; `960215-1` 3,863 to 3,673).

What still does not run, per reason:

| corpus | refused or faulted | programs |
|---|---|---:|
| GCC | `llvm.returnaddress` (`20030323-1`, `20030811-1`, `pr17377`); agree since, run alone (section above) | 3 |
| GCC | `llvm.eh.sjlj.longjmp` (`built-in-setjmp`, `pr84521`); agree since, run alone (section above) | 2 |
| GCC | `longjmp` (`20210505-1`), `setjmp` (`pr56982`): musl's are x86-64 assembly, not built; agree since, run alone | 2 |
| GCC | inline assembly (`990413-2`) | 1 |
| GCC | a variadic argument aligned to more than 16 bytes (`pr92904`); agrees since, run alone (section above) | 1 |
| GCC | named fault `unreachable`: no file system (`printf-2`, `user-printf`, `fprintf-2`, `loop-2f`, `loop-2g`) | 5 |
| GCC | named fault `bad_indirect`: unprototyped call with an extra argument (`pr67037`) | 1 |
| c-testsuite | a call through a mismatched prototype of `strlen` (`00025`) | 1 |
| c-testsuite | named fault `unreachable`: `fopen` (`00187`) | 1 |
| c-testsuite | the 10^8-step budget (`00040`) | 1 |
| `ieee/` | `cdivchkf`, `cdivchkd`, `cdivchkld`: musl's `ilogb.c`, `ilogbl.c` under `FENV_ACCESS` | 3 |

The first pass of the GCC run reported `20070614-1` and `complex-5` as crashes ("cannot read the
complex-arithmetic runtime"): the front now loads `complex-runtime.bc` beside its binary, and it had not been
copied with the others. With it copied, those two were run again with the same binaries and library.

## The programs that waited on the complex helpers (2026-11-15)

After compiler-rt's complex multiply and divide helpers were vendored (`rt/compiler-rt`), only the programs
refused for want of them were run again, with `run-all.sh` on a directory holding just them, 3 at a time,
with musl linked (`MUSL_LIBDIR`) and without:

| program | before | with musl | without the library |
|---|---|---|---|
| GCC `20070614-1` | refused: `__mulsc3` | agrees (3,662 steps) | agrees (1,613 steps) |
| GCC `complex-5` | refused: `__divsc3` | agrees (3,263 steps) | agrees (1,222 steps) |
| `ieee/` `cdivchkf` | refused: `__divsc3` (with musl: at `ilogb`) | refused at musl's `ilogb.c` (`FENV_ACCESS`) | refused: `ilogb` has no definition |
| `ieee/` `cdivchkd`, `cdivchkld` | refused at musl's `ilogb.c`, `ilogbl.c` | unchanged | refused: `ilogb`, `ilogbl` have no definition |

The three `cdivchk` programs call `ilogb` themselves. Given a test-only `ilogb` computed from the bits, all
three abort on the device (status 134), as they do natively when built with `clang --rtlib=compiler-rt`:
compiler-rt's division misses their accuracy bound, libgcc's (the native reference here) meets it
(`rt/compiler-rt/README.md`, "Not libgcc").

## Results with the C library linked (musl 1.2.5, 2026-11-15)

Each program is linked with the musl runtime (`rt/musl/link.py`: the program, musl's own startup and exit code,
the archive members a static linker would pull, and the device's system calls), run from
`__bennett_entry`, and compared with the native program on exit status **and standard output**. Native is
clang 18 with the system's glibc; its stdout goes to a file and its stdin is `/dev/null`, as the device's
console and empty stdin. Set `MUSL_LIBDIR` to a library built by `rt/musl/build.py` to select this mode in
`run-one.sh`; without it the harness is the one described below (`rt.c`, exit status only). In this mode a
program's own `abort()` or `exit(n)` reaches musl, so a failing self-check is a status other than 0 (abort's
is 134); variants A and B build the same program, so a fault the program raises is reported as a fault. Programs whose native stdout differs between `-O0` and
`-O2` are skipped (one GCC program). An "agrees" with output that differs from native is counted as wrong.

Method: the tree at `6da7ef6` exported with `git archive`, `bennett-front` and its float runtime and the
release `bennett` copied from builds of the same sources, the library built from the exported tree; 6
programs in parallel. The baseline is the same snapshot without the library. Wall time with the library:
GCC 321 s, c-testsuite 49 s, `ieee/` 14 s (baseline 276 s, 18 s, 8 s, measured before `zero N`). Two earlier
runs of this work, before it moved onto the constructor-order change and `zero N`, gave the same outcome and
detail for every program: one before the `fenv` change (which no corpus program uses) and one after it, whose
step counts are also unchanged now (its wall time was GCC 492 s, c-testsuite 66 s, `ieee/` 18 s). A run
after the init/fini bounds were kept undefined through the optimiser (`docs/DESIGN.md`) gave the same
totals and the same refusal and fault detail (GCC 323 s, c-testsuite 53 s, `ieee/` 21 s); the one GCC program
with a constructor (`20001229-1`) and a musl `printf` hello world (3,718 steps) lower to the same device text
as before it.

| outcome | GCC: baseline | with musl | c-testsuite: baseline | with musl | GCC `ieee/`: baseline | with musl |
|---|---:|---:|---:|---:|---:|---:|
| agrees | 1,521 | **1,557** | 151 | **217** | 58 | **64** |
| refused by name | 52 | 12 | 68 | 1 | 9 | 3 |
| named fault (native passed) | 1 | 6 | 0 | 1 | 0 | 0 |
| **wrong** | **0** | **0** | **0** | **0** | **0** | **0** |
| crash | 1 | 0 | 0 | 0 | 0 | 0 |
| budget | 0 | 0 | 1 | 1 | 0 | 0 |
| skipped | 81 | 81 | 0 | 0 | 1 | 1 |

No program moved from agrees to anything else. GCC: 35 refusals now agree, 5 became named faults, 12 stay
refused, and the crash (`20051113-1`, LLVM's dead-store-elimination bug of cause 1) does not occur when musl's
string functions are linked in place of `rt.c`'s loops; it agrees. The musl startup and exit sequence adds
about 2,050 device steps to a program (median over the programs that agree both ways: 2,047 GCC, 2,047
c-testsuite, 2,041 `ieee/`).

**The prediction against the measurement**, per refusal reason of the baseline (the prediction was 44 of 53
GCC refusals removed, 66 of 68 on c-testsuite, all 9 on `ieee/`):

| corpus | baseline refusal | programs | with musl |
|---|---|---:|---|
| GCC | `printf` | 14 | agree |
| GCC | `sprintf` | 9 | agree |
| GCC | `sprintf` through a pointer (`930513-1`) | 1 | agrees |
| GCC | `vprintf` 2, `snprintf` 1, `puts` 2 | 5 | agree |
| GCC | `stdout` | 4 | 2 agree (`fprintf-1`, `vfprintf-1`); 2 named faults (`printf-2`, `user-printf` open a temporary file) |
| GCC | `stderr` (`gofast`) | 1 | agrees (it writes to it only when a check fails) |
| GCC | `open` (`loop-2f`, `loop-2g`: `/dev/zero`) | 2 | named faults: no file system |
| GCC | `fopen` (`fprintf-2`) | 1 | named fault: no file system |
| GCC | `pow`, `isprint`, `qsort` | 3 | agree |
| GCC | `__mulsc3` (`20070614-1`), `__divsc3` (`complex-5`) | 2 | refused: no definition (the open choice of where the complex helpers come from) |
| GCC | `longjmp` (`20210505-1`), `_setjmp` (`pr56982`, now refused at `setjmp`) | 2 | refused: musl's `setjmp` and `longjmp` are x86-64 assembly, not built |
| GCC | `llvm.returnaddress` 3, `llvm.eh.sjlj.longjmp` 2, inline assembly 1, `float` to `i128` 1, an over-aligned variadic argument 1 | 8 | refused, unchanged (not library calls) |
| c-testsuite | `printf` 42, `puts` 22, `sprintf` 1, `stdout` 1 | 66 | agree |
| c-testsuite | `fopen` (`00187`) | 1 | named fault: no file system |
| c-testsuite | `strlen` declared as `int strlen(char *)` (`00025`) | 1 | refused, unchanged: a call through a mismatched prototype |
| `ieee/` | `signal` (`fp-cmp-1`, `-2`, `-3`) | 3 | agree: the handler is installed and never raised |
| `ieee/` | `sqrt`, `sin`, `sprintf` | 3 | agree |
| `ieee/` | `ilogb` (`cdivchkd`), `ilogbl` (`cdivchkld`), `__divsc3` (`cdivchkf`) | 3 | refused at musl's `ilogb.c`: `#pragma STDC FENV_ACCESS ON` gives a constrained operation the front refuses; they also divide complex numbers |

So GCC: 40 of 52 refusals removed (35 agree, 5 named faults on files), against 44 predicted; the four the
prediction counted and the measurement does not are `__mulsc3`, `__divsc3`, `longjmp` and `_setjmp`.
c-testsuite: 67 of 68 removed (66 agree, 1 named fault), against 66 predicted. `ieee/`: 6 of 9, against 9
predicted; the three complex-division checks wait on `FENV_ACCESS` and the complex helpers.

**Named faults that remain, and what removes them.** File I/O (`open`, `fopen`, `freopen` of temporary files;
6 programs) traps by name: it needs a file model in the runtime. `pr67037` is the unprototyped call with an
extra argument described below (`bad_indirect`), as before. c-testsuite `00040` runs out of the 10^8-step
budget, as before.

**What the corpus run found before it was clean** (all fixed, each with a regression test; details in
`rt/musl/README.md`): musl's `malloc` trapped on its first call (a fixed `PROT_NONE` guard page); `abort`
trapped (musl's `raise` blocks the signal around `tkill`); reads of stdin through `readv` trapped; every
program that used `scanf` ended in `bad_indirect` at exit, a front bug: `scanf`'s alias made a variadic
function address-taken, and the front then lowered every call through an unprototyped pointer (musl's call
of each destructor) as a variadic call. The front now compares the pointer with each address-taken variadic
function at the call. A harness bug left `run-one.sh` without `here`, and two library build problems made a
fresh library resolve nothing (a stale symbol cache) or crash six concurrent links (one shared partial file).

## Results after floating point (2026-11-15)

`float`, `double` and `long double` now run as calls of Berkeley SoftFloat 3e (`rt/softfloat/README.md`,
`docs/DESIGN.md`). Measured with the same harness, binaries built once from clean commits and copied: before at
`b875989` (it reproduces the step-5 numbers below exactly), after at `167bafe`. New in the harness:
`EXTRA_FLAGS` (options for every test of a corpus), a test's `.x` file may add `-fno-trapping-math`, and `KEEP=0`
deletes each program's outputs once its result line is written. GCC's `gcc.c-torture/execute/ieee/` directory
(68 programs, fetched with the same sparse checkout) is added as a floating-point corpus and run with
`EXTRA_FLAGS=-fno-inline`, as its `ieee.exp` does.

| outcome | GCC: before | after | GCC, `OPTNONE=1`: before | after | c-testsuite: before | after | GCC `ieee/`: before | after |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| agrees | 1,402 | **1,521** | 1,407 | **1,523** | 149 | **151** | 12 | **58** |
| refused by name | 172 | 53 | 168 | 52 | 70 | 68 | 55 | 9 |
| named fault (native passed) | 1 | 1 | 0 | 0 | 0 | 0 | 0 | 0 |
| **wrong** | **0** | **0** | **0** | **0** | **0** | **0** | **0** | **0** |
| crash | 1 | 1 | 1 | 1 | 0 | 0 | 0 | 0 |
| budget | 0 | 0 | 0 | 0 | 1 | 1 | 0 | 0 |
| skipped | 80 | 80 | 80 | 80 | 0 | 0 | 1 | 1 |

(The `OPTNONE=1` "before" column is the step-5 measurement, of the same code.) The crash, the named fault and
the budget case are the ones described below. GCC: 120 programs moved from refused to agrees, one from agrees to
refused, none in any other direction. Of the 120, 13 take fewer than 10 device steps, 44 take 10–99, 37 take
100–999, 25 take 10^3–10^5 and one (`pr28982b`) 1.6·10^6. `ieee/`: 46 moved from refused to agrees (5, 9, 17
and 15 in the same step ranges). The one that moved the other way, `990413-2`, uses x87 inline assembly
(`fpatan`, `fsqrt`) inside its own static `asin`. It agreed before only because LLVM took that function for the
C library's `asin` and folded the whole program away; once its signature is integers that no longer happens,
and the assembly is refused by name, as any non-empty assembly is.

**Where the 127 floating-point refusals of GCC went.** 119 agree. 8 are refused for another reason: 5 wait on
the C library (`sprintf` 3, `puts`, `pow`), 2 on the complex multiply and divide helpers `__mulsc3`/`__divsc3`
(libgcc names, but their Annex G recovery needs the C library's `logb` and `scalbn`), and 1 converts a `float`
to `__int128` (SoftFloat has no 128-bit integer conversions). c-testsuite: of 4, 2 agree and 2 wait on
`printf`. `ieee/`: of 50, 45 agree and 5 wait on the C library (`sqrt` called as a function under the default
`-fmath-errno`, `sin`, `ilogb`, `ilogbl`, `sprintf`); its other 4 refusals are `signal` (3) and `__divsc3`.
No refusal in any corpus is "floating point" in general.

**Wrong results during the work: none on the corpora.** The tests and one independent testing pass (by a
different model family, aimed only at wrong answers: differential runs against native on random operands for
every operation and conversion, and programs mixing floats with variadics, structs by value, unions, function
pointers, vectors, in-place updates and coherent regions) found two, both fixed with regression tests: an
out-of-range conversion of a `double` or `float` to `unsigned char` or `unsigned short` (1e10 gave 0xe400, native
0), and `fmin(+0, -0)`, which was -0 on the device and +0 natively. Both are cases where C leaves the result
open and the toolchain follows the native program.

### Refusal reasons after floating point (GCC, README pipeline, 53 refusals)

| count | family |
|---:|---|
| 44 | library not provided: `printf`-family 29 (`printf` 15, `sprintf` 9, `vprintf` 2, `snprintf`, `puts` 2), `stdout`/`stderr` 5, `open` 2, `pow`, `__mulsc3`, `__divsc3`, `fopen`, `qsort`, `isprint`, `longjmp`, `_setjmp` |
| 3 | `llvm.returnaddress` |
| 2 | `llvm.eh.sjlj.longjmp` |
| 1 | x87 inline assembly (`990413-2`) |
| 1 | a conversion from `float` to `i128` (`pr49218`) |
| 1 | a variadic argument aligned to more than 16 bytes (`pr92904`) |
| 1 | a call with variadic arguments through a pointer, where no variadic function is defined (`930513-1`) |

## Results after step 5

| outcome | GCC, README pipeline: before | after | GCC, `OPTNONE=1`: first | after | c-testsuite: before | after |
|---|---:|---:|---:|---:|---:|---:|
| agrees | 1,241 | **1,402** | 1,244 | **1,407** | 146 | **149** |
| refused by name | 334 | 172 | 331 | 168 | 73 | 70 |
| named fault (native passed) | 0 | 1 | 0 | 0 | 0 | 0 |
| **wrong** | **0** | **0** | **0** | **0** | **0** | **0** |
| crash | 1 | 1 | 1 | 1 | 0 | 0 |
| budget | 0 | 0 | 0 | 0 | 1 | 1 |
| skipped | 80 | 80 | 80 | 80 | 0 | 0 |

"Before" is `b5ffb85` (the same numbers as `4b65247`, except that one c-testsuite program, `00025`, moved from the
named fault `bad_indirect` to a refusal; for `OPTNONE=1` the column is the first measurement at `4b65247`). The
crash is still the LLVM 18 dead-store-elimination bug of cause 1, which `bennett-front` now reports by name
(exit status 2, "LLVM's optimiser crashed on this module"); the harness counts any exit status other than 3 as a
crash. The named fault is GCC `pr67037`: a call through an unprototyped (K&R) function pointer that passes an
argument its target does not take. Natively the extra argument is ignored; on the device the call's signature
does not match the target's, and the run ends in `bad_indirect`. It was a refusal before. With inlining
(`OPTNONE=1`) it agrees.

GCC, README pipeline: 161 programs moved from refused to agrees, one (`pr67037`) from refused to a named fault,
none in any other direction. Of the 161, 9 take fewer than 10 device steps (folded at compile time), 92 take
10–99, 39 take 100–999, 15 take 10^3–10^5 and 6 take more. By the refusal they had before: empty inline
assembly 56, variadic functions 36, aggregate values 17, intrinsics 17, vectors 13, integers wider than 64 bits
9, computed goto 7, constant expressions on addresses 6.

**Wrong results during the step.** The first re-run (variadic functions only) gave 6 wrong results, GCC
`va-arg-1`, `va-arg-2`, `va-arg-8`, `va-arg-9`, `va-arg-10`, `va-arg-23`, all from one cause: named parameters
beyond the registers (more than six, or a named struct passed in memory) take stack slots, the caller placed the
variadic arguments after them, but `va_start` pointed the `va_list` at the start of the stack arguments. Fixed
(`va_start` starts after the named parameters' stack bytes); `front/tests/c/variadic.c` checks 16 and 17 fail
on the old code. No later run had a wrong result.

### Refusal reasons after step 5 (GCC, README pipeline, 172 refusals)

| count | family |
|---:|---|
| 127 | floating point (`double` 58, FP constants 32, `float` 20, `x86_fp80` 11, conversions and `fcmp` 5, a variadic `<2 x float>` 1) |
| 37 | library not provided: `printf`-family 25 (`printf` 15, `sprintf` 6, `vprintf` 2, `snprintf`, `puts`), `stdout`/`stderr` 5, `open` 2, `fopen`, `qsort`, `isprint`, `longjmp`, `_setjmp` |
| 3 | `llvm.returnaddress` (GCC's `__builtin_return_address`) |
| 2 | `llvm.eh.sjlj.longjmp` |
| 1 | a `bitcast` left from a vector of doubles (`pr92618`) |
| 1 | a variadic argument aligned to more than 16 bytes (`pr92904`: natively the stack is 16-byte aligned at a call) |
| 1 | a call with variadic arguments through a pointer, where no variadic function is defined (`930513-1`: `sprintf` through a pointer) |

Outside floating point and the C library, 6 programs remain: GCC's return address (3), its `setjmp`/`longjmp`
builtins (2) and the over-aligned variadic argument. c-testsuite: 70 refusals: 64 library calls (`printf`, `puts`
and other stdio), 4 floating point, 1 call through a mismatched prototype (`00025`, cause 2 of the first
measurement) and 1 call of a library function through a pointer.

## Results at `4b65247` (first measurement)

### GCC torture `execute`, README pipeline (1,656 programs)

| outcome | count | % of all | % of not skipped |
|---|---:|---:|---:|
| agrees | 1,241 | 74.9 | 78.7 |
| refused by name | 334 | 20.2 | 21.2 |
| named fault or stop (native passed) | 0 | 0 | 0 |
| **wrong** | **0** | 0 | 0 |
| crash | 1 | 0.1 | 0.1 |
| budget | 0 | 0 | 0 |
| skipped | 80 | 4.8 | |

Skipped: 57 do not compile with clang 18 (GCC nested functions, variable-length arrays in structs, ...); 16 give
different results at native -O0 and -O2 (GCC-specific behaviour such as `__builtin_return_address`, `-finstrument-functions`,
`__builtin_*_chk`, or undefined behaviour); 7 fail natively even at both levels.

### GCC torture `execute`, plain `clang -O0` bitcode (`OPTNONE=1`, 1,656 programs)

The same pipeline, without `-Xclang -disable-O0-optnone`. I meant this as a configuration with less folding, but
`bennett-front` removes `optnone` and `noinline` together (`front/src/Normalise.cpp:29-31`). It therefore inlines and
folds *more* than with the README flags, which keep clang's `noinline` on every function. So this is an "inlining
allowed" configuration: a second, independent lowering of the same programs.

| outcome | count | % of not skipped |
|---|---:|---:|
| agrees | 1,244 | 78.9 |
| refused by name | 331 | 21.0 |
| named fault | 0 | 0 |
| **wrong** | **0** | 0 |
| crash | 1 (the same LLVM crash, `20051113-1`) | 0.1 |
| budget | 0 | 0 |
| skipped | 80 | |

Against the README configuration, 8 programs move from refused to agrees: after inlining, the floating-point,
vector or wide constant that was refused folds away. 5 move from agrees to refused: inlining exposes a constant
`icmp` on global addresses (3), `llvm.usub.sat`, or inline assembly. No program moves to wrong, fault or crash.

### c-testsuite (220 programs)

| outcome | count | % |
|---|---:|---:|
| agrees | 146 | 66.4 |
| refused by name | 72 | 32.7 |
| named fault (native passed) | 1 | 0.5 |
| **wrong** | **0** | 0 |
| crash | 0 | 0 |
| budget | 1 | 0.5 |
| skipped | 0 | 0 |

The budget case (`00040`, an 8-queens search) finishes with result 0 after 489,692,701 steps when it is given a
budget of 2·10^9: it **agrees**, and simply needs more than 10^8 steps. The fault (`00025`) is the mismatched-
prototype cause below. 65 of the 72 refusals are `printf` (41 of the 43 calls of a variadic function, and 22 calls
of `puts`, which LLVM makes from `printf("...\n")`) or other stdio.

### Refusal reasons at `4b65247`: the work list for step 5 (GCC, README pipeline, 334 refusals)

By family:

| count | family |
|---:|---|
| 101 | floating point (`double` 49, `float` 17, FP constants 15, `x86_fp80` 8, `<2 x float>` 6, `sitofp`/`uitofp`/`fcmp` 5, `{double,double}` 1) |
| 90 | variadic: definitions of variadic functions 66; calls of variadic functions 24 (13 are `printf`-family; the other 11 are calls of `open`, through function pointers, or through unprototyped K&R declarations, which clang emits as variadic calls) |
| 57 | inline assembly (56 of them are an empty `asm("" ...)`, used as an optimisation barrier) |
| 20 | aggregate values in registers: small structs returned or passed as `{ i64, i64 }` (10), `{ i64, i32 }` (4), `{ i64, i48 }`, `{ ptr, i64 }`, `{ i8, i64 }`, `{ i32, i64 }`; one `extractvalue`; one `zeroinitializer` constant |
| 17 | vector values (`<2 x i32>`, `<4 x i16>`, ..., `extractelement`/`insertelement`, vector constants): GCC vector extensions |
| 15 | other intrinsics: `llvm.prefetch` 6, `llvm.stacksave` 3, `llvm.returnaddress` 2, `llvm.eh.sjlj.longjmp` 2, `llvm.trap` 1, `llvm.bitreverse` 1 |
| 9 | integers wider than 64 bits (`i128` 5, `i65` 2, 72-bit constants from bit-field packing 2) |
| 8 | library not provided: `stdout`/`stderr` 3, `puts`, `longjmp`, `isprint`, `fopen`, `qsort` |
| 6 | computed goto (`indirectbr` 4, `blockaddress` 2) |
| 6 | overflow and saturating intrinsics: `llvm.smul.with.overflow` 3, `llvm.uadd.with.overflow` 2, `llvm.usub.sat` 1 |
| 5 | constant expressions on addresses: `ptrtoint` of a global truncated to `i32`/`i8` and added to (3), `icmp` of such (1), `ptrtoint(blockaddress)` (1) |

The full ranked list of normalised reasons comes from `python3 summarize.py <results.tsv>`.

Cheapest items for ordinary code, in my estimate: the overflow intrinsics (`umul/smul/uadd.with.overflow`: LLVM
creates them from ordinary overflow checks; this runtime's own `calloc` was refused for `count > SIZE_MAX / size`
until the check was removed), `llvm.prefetch` (a no-op), `llvm.stacksave`/`stackrestore`, aggregate values of
integers (registers holding structs), and calls through unprototyped declarations whose definition is visible.
The large families are floating point (101), variadic functions (90) and inline assembly (57).

## Every distinct crash and fault cause (first measurement)

**1. `bennett-front` segfaults in LLVM's dead-store elimination** (crash; GCC `20051113-1`; the same in the optnone
run). Reduced by hand (`reduced/dse-calloc-crash.c`, 7 lines): a module that defines `calloc` and a non-inlinable
`malloc`, and calls `memset` on the result of `malloc`. Expected: a device program, or a refusal by name. Observed: exit
139, stack in `llvm::DSEPass::run` → `MemorySSAUpdater::createMemoryAccessAfter`, the malloc + memset → calloc fold.
Stock `opt-18 -passes='default<O2>'` crashes on the same module (and on `llvm-reduce`'s 20-line reduction in
`.tmp/c-corpus/reduce/calloc/reduced.ll`). Cause: an upstream LLVM 18.1.3 bug, reached through
`bennett::optimise` (`front/src/Normalise.cpp`), not in Bennett's code (high confidence). Removing the `calloc`
definition, or only declaring it, makes it disappear. The trigger here is this experiment's runtime, which defines
`calloc`; a real program with its own allocator would hit it the same way. `bennett-front` has no guard, so it
crashes rather than refusing.

**2. A call through a mismatched prototype is not refused; the run ends in `bad_indirect`** (named fault;
c-testsuite `00025`, which declares `int strlen(char *)` against the runtime's `size_t strlen(const char *)`). Reduced
by hand: `reduced/mismatched-prototype.c` (`((int (*)(int))f)(5)` with `long f(long)`). Expected (by `front`'s own
code): `not handled yet: a call through a mismatched prototype of @f`. Observed: `bennett-front` writes
`call_indirect fn(i32) -> i32 1(i32 5)`, and the run ends in fault `bad_indirect`. Native x86-64 returns 0.
Cause: in `front/src/Emit.cpp` (call emission, around line 686), `CallBase::getCalledFunction()` returns null whenever
the call's function type differs from the callee's, so the `callee->getFunctionType() != call.getFunctionType()`
check under it can never be true. The call falls through to the indirect-call path, with the constant function
address as target (high confidence). The rule holds (the fault is named), but the intended refusal is dead code, and
the fault names a symptom (`bad_indirect`) rather than the cause. When the device widths of the two prototypes happen
to agree, the call runs, which is ABI-faithful.

**Wrong results: none**, in either corpus or either configuration.

**Named faults where native succeeded:** only the one above. No `out_of_bounds`, `stack_overflow`, `unreachable` or
`divide_by_zero` from a program itself.

## Measured facts (first measurement)

- At `4b65247`, GCC torture `execute` through the README pipeline: 1,241 agree, 334 refused by name, 1 crash (an upstream
  LLVM pass), 0 wrong, 0 named faults, 0 budget, 80 skipped.
- c-testsuite: 146 agree, 72 refused, 1 named fault (cause 2), 1 budget (agrees at 2·10^9 steps), 0 wrong, 0 crash.
- Inlining-allowed configuration (`OPTNONE=1`): 1,244 agree, 331 refused, 1 crash (the same), 0 wrong, 0 named faults;
  859 of the 1,244 agreeing programs take fewer than 10 steps.
- Device steps of the agreeing GCC programs: 368 of 1,241 take fewer than 10 steps (all of `main` was folded to
  `ret 0` or `exit(0)` at compile time), 634 take 10–99, 142 take 100–999, 74 take 10^3–10^5, and 18 take more. In
  c-testsuite, 127 of 146 take fewer than 10.
- No program hung or exhausted memory in `bennett-front` or `bennett`.

## Interpretation, and what would make these numbers misleading

- **Much of "agrees" tests LLVM, not `bennett-front`.** The pipeline runs LLVM's O2 inside `bennett-front`, and for
  30% of the agreeing GCC tests (86% in c-testsuite; 69% in the inlining configuration) the whole check folds away
  before lowering. Those programs show only that nothing was broken on the way. Short of changing the compiler,
  I found no way to switch off the O2 inside `bennett-front`, so the 873 agreeing GCC programs that run 10 or more
  steps are the real test of the lowering. That is far more than the 15-program hand corpus. I did not measure
  whether this corpus would have caught the three earlier silent errors: that needs the pre-fix commit `70cfd37`
  built, and this task did not rebuild. Only 9 of those 873 have struct, align, bitfield or init in their names.
- **Refusal hides behaviour.** 21% of GCC tests and 33% of c-testsuite tests never reach the device. Floating point,
  variadic functions and `printf` take out exactly the kind of code where wide values and struct passing go wrong.
  Bugs in those areas are untested until they are no longer refused.
- **The oracle is only the exit status.** A wrong value that the test does not check, or that the device happens to
  map to 0 mod 256, passes. c-testsuite's expected output is not compared (programs that print are refused anyway).
- **The runtime is mine.** A bug in `rt.c` would show up as a fault or wrong result, not hide one, except that `exit(0)`
  and `abort` are recognised by fault kind. The A/B swap makes misattribution need two coincident faults.
- **One target and one clang.** Native and device both go through LLVM 18, so a bug they share (as in cause 1) is
  invisible where it miscompiles instead of crashing. Tests are not compiled the way GCC's harness compiles them
  (only a whitelist of `dg-options` is honoured; GCC optimisation-level torture is not reproduced).
- **Harness fixes during the run** (each re-run in full or for the affected subset): linking only the runtime functions
  that were needed missed the calls LLVM creates later (`calloc`, `bcmp`), and so refused tests that were fine; the
  `dg-options { "-fwrapv" }` brace form and `dg-additional-options` (73 tests use `-std=gnu89`) were first ignored. The
  latter made `930529-1` loop forever natively at -O2 until it was killed by hand. The tables are from the corrected
  harness.

## Files

- `rt.c`: the runtime. `run-one.sh`: one program, prints one classified line. `run-all.sh <corpus dir> <work dir>
  [jobs]`: all top-level `*.c` of a directory, binaries from `<work dir>/../bin` (or `BIN`), `OPTNONE=1` for the stress
  configuration. `summarize.py <results.tsv>`: the tables. `reduced/`: the two hand-written reduced programs.
- Per-program outputs (logs, `.bn`, native binaries) are in `.tmp/c-corpus/<work dir>/out/<test>/`. The final GCC
  table is `.tmp/c-corpus/gcc/results-final.tsv` (the full run plus the option-fix rerun in `gcc-opts/`).
