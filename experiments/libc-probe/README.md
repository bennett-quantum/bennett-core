# libc probe: what does a real C library need from Bennett, and how far does it get today?

The question: of the 172 refusals on GCC's C torture tests, 37 are C library calls, and on c-testsuite 64 of 70.
They are refused because the harness of `experiments/c-corpus` supplies no C library. The planned route is a real C
library compiled to bitcode and linked in, with the system-call boundary mapped to device effects. Which library,
and where the boundary sits, is the maintainer's decision. This experiment measures what that decision needs: what
compiles, what `bennett-front` refuses and why, the smallest substitutions that get programs running, whether their
output is right, what it costs, and what the boundary would need from the device. It builds no product; nothing in
`crates/` or `front/` is changed.

Measured on 2026-11-15 with `bennett-front` and `bennett` built from commit `b875989` (tree clean, `front/test.sh`
passing) and copied to `.tmp/libc-probe/bin/`; only those copies were used, except for the numbers labelled
**front-effect**, which used a separate binary built from `b875989` plus `front-effect.patch` (section 6). clang
18.1.3, LLVM 18. musl 1.2.5 (the release archive). picolibc 1.8.12 (tag `1.8.12`, commit `2ae376c6`, from GitHub).

## Method

**Libraries.**

- `musl-bc.sh <out> [overlay dir ...]` extracts `MUSL_TARBALL`, runs musl's `configure --target=x86_64`, asks musl's
  own Makefile for its object list (arch files replace generic ones of the same name, as in a normal build) and
  compiles every source to bitcode with musl's own flags (`-std=c99 -nostdinc -ffreestanding
  -fexcess-precision=standard -frounding-math -fno-strict-aliasing ...`, musl's include order) plus the README's
  `-O0 -Xclang -disable-O0-optnone -gline-tables-only -emit-llvm`. Overlay directories come before musl's arch
  headers; `GENERIC=1` compiles musl's generic C file in place of an x86_64 assembly or inline-assembly file of the
  same name where one exists; `MUSL_PATCHES` are applied to the extracted sources.
- `picolibc-bc.sh <out>` copies `PICOLIBC_DIR`, applies `PICOLIBC_PATCHES`, runs `meson setup` only (meson 1.12.1
  and ninja from a venv under `.tmp/`: `python3 -m venv`, `pip install meson ninja`), and `picolibc-cmds.py` turns
  meson's compile database into one bitcode compile per source of `libc.a`, the x86 machine library and
  `libstrcmp.a`, with picolibc's flags minus `-Oz -g -ffunction-sections` and plus the README's flags. Configuration:
  tinystdio with `format-default=long-long` (integer printf and scanf with `long long`), `posix-console` (stdin,
  stdout, stderr over `read`/`write`), `stdio-exit-flush`, `single-thread`, no thread-local storage, no semihosting,
  `atomic-ungetc=false` (see 5). The 7 x86 assembly string functions are replaced by picolibc's generic C files.

**Link.** `resolve.py` does what a static linker does with an archive: starting from the program and the probe
runtime, it pulls the members that define a symbol still undefined, transitively; a strong definition is preferred
to a weak one; an undefined weak reference pulls nothing. Linking the whole library as one module instead (the
previous attempt here, `llvm-link --only-needed` of a merged module) is wrong for musl: musl gives many symbols a weak
dummy in one file and the strong version in another that is linked only when something else needs it
(`__syscall_cp`, `__stdio_exit`, ...). Merged, the strong ones always win, and every hand program was refused at
`the global @__cp_begin, which has no definition` (thread cancellation, from `pthread_cancel.c`). Then two steps a
static linker and a C runtime's startup do and `llvm-link` does not:

- `ctors.py`: the entries of `@llvm.global_ctors`/`@llvm.global_dtors` (and function pointers a library places in
  `.init_array*`/`.fini_array*` itself, as picolibc does for its `atexit` handlers) become ordinary arrays the probe
  runtime calls. Without this, constructors silently do not run (finding 1 below).
- `weak.py`: undefined weak references become null, as `ld` resolves them. picolibc tests `&stdin != NULL` this way;
  `bennett-front` refuses every global without a definition, weak or not.

After that, two link modes. `LINK=external`: the module goes to the front as linked. `LINK=internalize` (the
default of the scripts, used for the hand programs): every symbol except the entry is made internal first, so that
the front's O2 can drop what nothing calls, as `--gc-sections` would, and inline across the library boundary, as
LTO would. A refusal `a call to @f, which has no definition` where the library defines `f` (a call LLVM's optimiser
created, e.g. `puts` from `printf("...\n")`) adds `f` as a root and links again. Each library member is one source
file, so a member brings every function of its file.

**Probe runtime** (`musl-rt.c`, `picolibc-rt.c`). The device has no operating system, no console instruction
reachable from C (section 6), and a run ends only by returning from the entry function or by a named fault. The
runtime is the boundary: for musl one C function `__bennett_syscall(n, a1..a6)` that the overlay `syscall_arch.h`
calls for every system call; for picolibc its porting layer (`write`, `read`, `_exit`, `sbrk`, `open`, `close`,
`lseek`, `fstat`, `isatty`, `getpid`, `kill`, `unlink`). Writes to fd 1 append to a 64 KiB buffer; fd 2 is dropped;
fd 0 is empty. `brk`/`mmap` (musl) or `sbrk` (picolibc) carve a 1 MiB static arena. Opening a file is a trap, the
named fault `unreachable` (no file system), unless `FILES_ENOENT` is defined, when it fails with `ENOENT`. Any other
musl system call is a trap. The entry `__bennett_entry` does the startup (thread descriptor, `environ`, constructors),
calls `main` with the parameters it declares (`argc = 1`, `argv = {"prog", 0}`), then what `exit` does (atexit
handlers, destructors, stdio flush), and returns the result.

**Oracle: output and status, not status alone.** The result is a 64-bit FNV-1a hash of (the bytes written to fd 1,
then one byte, the exit status mod 256); `hash.py` computes the same from the native program's real stdout and status.
On return from `main` the device's `result:` is the hash and is compared with native. A run that ends in `exit` or
`abort` cannot return, so the runtime compares the hash with the native one compiled into it and ends in a fault that
says which, swapping the fault kinds between two builds as `experiments/c-corpus/rt.c` does (variant A: match ->
`bad_indirect`, mismatch -> `divide_by_zero`; variant B the other way round; if the kind does not swap, the program
itself faulted). The self-tests in `selftest/` check it: a program whose output depends on whether a stack address is
above 2^40 is classified **wrong** on both the return and the exit path; one with equal output agrees. c-testsuite's
`.expected` files were compared with native stdout: all 220 equal.

**Per program** (`probe-one.sh`, fanned out by `probe-all.sh`, 40 wide): native `clang-18 -O0` and `-O2` against the
system C library, which must agree on stdout and status (no time limit beyond 60 s; a native failure is *not* a skip:
the device must reproduce it); device: clang with the README's flags against the library's own headers, link as
above, `bennett-front --region-budget 100000 --stack-bytes 1048576` under `/usr/bin/time -v` and a 900 s timeout,
`bennett check`, `bennett run --entry __bennett_entry --budget 1000000000`. Every stage runs under the c-corpus
harness's 2 GiB memory cap without swap. Dialect options from GCC's `dg-options` as in c-corpus. Classes as in
c-corpus, with **agrees** now meaning output and status equal native, and **wrong** meaning the run succeeded with
different output or status.

**Corpora**: GCC 14.2.0 `gcc.c-torture/execute` (1,656 programs, commit `04696df0`) and c-testsuite `single-exec`
(220, commit `5c727565`), not copied into the tree. The lists of the 37 and 64 library refusals come from reproducing
the c-corpus baseline at `b875989` (GCC 1,402 agree / 172 refused / 37 of them "no definition"; c-testsuite 149 / 70
/ 64): the same numbers as published.

## 1. musl 1.2.5 as bitcode under clang 18

musl's Makefile selects 1,345 sources for x86_64: 1,315 C files and 30 assembly files. **All 1,315 C files compile
to bitcode as is**, with musl's headers and flags. What they and the 30 others contain that the front cannot take:

| what | sources (as musl builds them) | substitute in this probe | without it the front says |
|---|---|---|---|
| system calls as inline assembly (`arch/x86_64/syscall_arch.h`, `__syscall0`..`6`) | 260 files with 325 `syscall` asm statements; 284 C files use the syscall macros; 288 distinct `SYS_` numbers named | `musl-overlay/syscall/syscall_arch.h`: each `__syscallN` calls the C function `__bennett_syscall`; no vDSO | `inline assembly, at ./arch/x86_64/syscall_arch.h:38` |
| atomics (`arch/x86_64/atomic_arch.h`: `lock cmpxchg`, `xchg`, `lock or`, `pause`, `bsf`, ...) | 52 files, 101 asm statements; 53 to 57 C files use `a_*` (text search; the count depends on the pattern) | `musl-overlay/atomic/atomic_arch.h`: `a_cas`/`a_cas_p` as a plain read and write (one thread), `a_crash` a trap; musl derives the rest from `a_cas` | `inline assembly, at ./arch/x86_64/atomic_arch.h:4` |
| thread pointer (`%fs:0`, `__get_tp`) | 66 files (51 via `__pthread_self()`/`CURRENT_LOCALE`, e.g. `errno`, locale) | `musl-overlay/tp/pthread_arch.h`: the address of one static descriptor that the runtime fills | `inline assembly, at ./arch/x86_64/pthread_arch.h:4` |
| assembly sources | 30: x87 `long double` libm 15 (`acosl` ... `truncl`, `__invtrigl`), `memcpy`/`memmove`/`memset`, `setjmp`/`longjmp`/`sigsetjmp`, `syscall_cp`/`clone`/`__set_thread_area`/`__unmapself`, `vfork`, `restore` (signal return), `fenv`, `dlsym`, `tlsdesc` | `GENERIC=1`: musl's generic C file of the same name (all 30 have one; 4 are empty: `setjmp`, `longjmp`, `sigsetjmp`, `syscall_cp`) | `a call to @memset, which has no definition` |
| x87/SSE inline assembly in arch C files | 14 files (`sqrt`, `sqrtf`, `sqrtl`, `fabsl`, `fmodl`, `remainderl`, `remquol`, `rintl`, `lrint*`, `llrint*`) | `GENERIC=1`: the generic C files (18 arch C files replaced) | inline assembly |
| startup | `crt1.c` (`_start` as 12 lines of top-level asm in `crt_arch.h`), `crti.s`/`crtn.s`, `__libc_start_main.c` (`__init_libc`, `__init_tls` from the program headers, `__init_tp` -> `__set_thread_area`) | `musl-rt.c`: `__bennett_entry` and `init()` set the descriptor's `self`, `tid`, `locale`, `robust_list`, `libc.auxv`, page size, `environ`, `__progname` | not reached: no `_start` |
| locks | 61 to 66 files (`LOCK`/`FLOCK`/`__lock`/`__lockfile`, text search as above); futex wait/wake in 20 | none needed: with one thread `libc.threaded` is 0 and the lock paths are skipped; `a_cas` is plain | |
| `setjmp`/`longjmp` | 3 assembly files, no C version | none | `a call to @setjmp, which has no definition` (by name) |
| malloc | mallocng (7 files) over `brk` and `mmap` (+ `munmap`, `madvise`, `mprotect`, `mremap`); `lite_malloc` (bump) behind `__libc_calloc` | the runtime's `brk` grows up, `mmap` down, in a 1 MiB static arena | |
| `.init_array`/`.fini_array` | `__libc_start_init`, `__libc_exit_fini` walk linker-provided bounds | `ctors.py` arrays; the runtime's strong `__libc_exit_fini` | constructors do not run (finding 1) |
| floating point | 305 of 1,345 files; 125 use `x86_fp80` (`long double`); 227 use `llvm.experimental.constrained.*` (musl builds with `-frounding-math`) | none (section 3) | `a value of type x86_fp80` |

With the three overlays and `GENERIC=1`, 1,345 of 1,345 sources compile; one file still has non-empty inline
assembly (`src/thread/__unmapself.c`, never pulled by any probe program). Each substitution is necessary: removing
any one of the three overlays makes the front refuse all 9 hand programs at that header's inline assembly; without
`GENERIC=1`, 8 of 9 are refused at `memcpy` or `memset` (no definition) and only `isprint` runs (directories
`.tmp/libc-probe/h-ab-*`). Per-file tables: `python3 classify.py <out> --table`.

## 2. What `bennett-front` does with it

### The hand programs, substitution by substitution (musl, `LINK=internalize`)

`programs/`: `puts`, `printf("hello %d\n", 42)` (`hello`), integer `printf` with flags, widths and `%ld %lld %llu %lx
%p` (`printf-int`, returns 3), `sprintf`, `qsort`, `isprint` over 256 characters, `fopen` of a missing file, `malloc`
/`free`/`realloc` (64 blocks), `atexit` + buffered output + `exit(5)` (`exit`), a constructor and a destructor
(`ctor`).

| step | substitution (cumulative) | agree | refused | named fault | first refusal or fault |
|---|---|---:|---:|---:|---|
| 0 | whole library as one merged module, three overlays | 0 | 9 | 0 | `the global @__cp_begin, which has no definition` (link artefact) |
| 1 | archive member pulling (`resolve.py`), overlays, no `GENERIC` | 1 | 8 | 0 | `a call to @memcpy` / `@memset`, which has no definition |
| 2 | + `GENERIC=1` | 5 | 4 | 0 | `a value of type x86_fp80, at src/stdio/vfprintf.c:627` (every `printf`-family program) |
| 3 | + `musl-printf-nofp.patch` | 9 | 0 | 0 | |
| 4 | + `musl-scanf-nofp.patch`, `open` traps | 8 | 0 | 1 | `fopen`: fault `unreachable` (no file system, by design) |
| 5 | + `ctors.py` | 9 of 10 | 0 | 1 | `ctor` added: agrees |

Every agreeing program printed exactly what native printed and ended with the same status, by `return` or by `exit`.
Steps and sizes are in section 4.

### The 37 GCC and 64 c-testsuite programs refused today for library calls

| musl configuration | GCC 37: agree | refused | fault | skipped | c-testsuite 64: agree | refused | fault |
|---|---:|---:|---:|---:|---:|---:|---:|
| step 2 (generic C, no patch; `open` fails with `ENOENT`) | 5 | 31 | 0 | 1 | 0 | 64 | 0 |
| step 3 (+ integer-only `printf`) | 30 | 6 | 0 | 1 | 61 | 2 | 1 (`00187`: `fopen` fails, the test uses the null `FILE`, `bad_indirect`) |
| step 5 (+ integer-only `scanf`, `open` traps, constructors) | **28** | 3 | 5 | 1 | **61** | 2 | 1 |

At step 2, 28 of the 31 GCC refusals and 62 of the 64 c-testsuite refusals are `x86_fp80` in `vfprintf.c`; the other
three GCC refusals are `setjmp`, `longjmp` and one program's own `float`, the other two c-testsuite refusals the
programs' own floating point. At step 3 the 3 new GCC refusals are `x86_fp80` in `vfscanf.c`
(`printf-2`, `fprintf-2`, `user-printf` read back a temporary file with `fscanf`). At step 5 those three end in the
named fault `unreachable` at system call 228, `clock_gettime`, which musl's `tmpnam` uses to make a name (the runtime
has no clock; found by printing the call number to the console with the front-effect binary), and `loop-2f`/`loop-2g`
(which `open("/dev/zero")` to `mmap` it) end there when they open a file. With `FILES_ENOENT` `loop-2f`/`loop-2g`
agree, because the tests skip their check when `open` fails, which tests less. The rest of the final state:

| program | outcome | why |
|---|---|---|
| `20210505-1` | refused | `a call to @longjmp, which has no definition` |
| `pr56982` | refused | `a call to @setjmp, which has no definition` |
| `gofast` | refused | `a value of type float` in the test itself |
| `return-addr` | skipped | native -O0 and -O2 print different output (`__builtin_return_address`) |
| c-testsuite `00174`, `00175` | refused | floating point in the test itself |
| c-testsuite `00187` | fault `unreachable` | writes and reads back a file |

The same holds with `LINK=external` (no internalisation): 28 / 3 / 5 / 1 and 61 / 2 / 1.

### The whole corpora against musl

Not asked, but the library changes every program's link, and agreement now includes output, so both corpora were
run in full (configuration of step 5).

| outcome | GCC: c-corpus baseline | musl, `LINK=external` | musl, `LINK=internalize` | c-testsuite: baseline | external | internalize |
|---|---:|---:|---:|---:|---:|---:|
| agrees (baseline: status only; probe: output and status) | 1,402 | **1,438** | 1,487 | 149 | **212** | 214 |
| refused by name | 172 | 138 | 89 | 70 | 7 | 5 |
| named fault (native passed) | 1 | 6 | 5 | 0 | 1 | 1 |
| **wrong** | 0 | **0** | **1** | 0 | **0** | **0** |
| crash | 1 | 0 | 0 | 0 | 0 | 0 |
| budget | 0 | 0 | 0 | 1 | 0 | 0 |
| skipped | 80 | 74 | 74 | 0 | 0 | 0 |

External, GCC: 28 library refusals now agree, 7 programs that fail natively (status 134, which c-corpus skips) are
reproduced exactly, and the LLVM crash of c-corpus cause 1 (`20051113-1`; the module now has musl's allocator instead
of `rt.c`'s, and LLVM does not crash on it) agrees; no program moved from agrees to anything else. The 138 refusals
are floating point 129 (all in the test programs, none in the library), `llvm.returnaddress` 3, `llvm.eh.sjlj.longjmp`
2, `setjmp`, `longjmp`, a `bitcast` of a vector of doubles and an over-aligned variadic argument. The 6 faults: the 5
of the list above (`clock_gettime` 3, `open` 2) and `pr67037` (c-corpus cause 2). The budget case of c-testsuite
(`00040`) finishes in 489,691,542 steps under the larger budget of this harness.

Internalisation adds 49 GCC agreements net: 47 floating-point refusals and 2 return-address refusals disappear
because, with every function internal, LLVM inlines and folds them away before lowering (as
c-corpus's `OPTNONE=1` showed on a smaller scale), and `pr67037`'s mismatched K&R call is inlined (fault to agrees); the vector `bitcast` (`pr92618`) stays refused. And it
moves one program from agrees to the
**one wrong result**, `bcp-1`, which is the probe's own fault, not the front's: the test checks that
`__builtin_constant_p(global)` is 0 for a global it marks `externally_visible` (a GCC attribute clang ignores); once
the probe makes `global` internal, LLVM proves it is never written and constant-folds it, and the test aborts. With
`LINK=external` it agrees. Reduced: `reduced/internalize-constant-p.c`. The lesson for a design: internalising a whole
program is an LTO assumption with a visible semantic edge (`__builtin_constant_p`, `externally_visible`, symbols used
from outside the module).

## 3. Floating point inside `printf`

**Granularity of a refusal** (`granularity/run.sh`, and `front/src`). `bennett-front` lowers every function that is
still defined in the module after its O2 pipeline (`front/src/main.cpp`, `ModuleWriter` in `front/src/Emit.cpp`),
and the first instruction it cannot lower ends the run with exit 3. There is no reachability analysis of its own.
So the unit is **the lowered instruction, over the whole module after O2**: an unused function with external
linkage that uses `double` is refused; the same function `static` is deleted by O2 and the program runs; `double` on
a branch that is never taken but not provably dead is refused; on a branch O2 proves dead it is not. For a C
library this means: a member file brings all its functions; `LINK=internalize` lets O2 delete the unused ones; what
remains reachable from a `switch` on a format character (`case 'f':` in `printf_core`) is lowered whatever the
program's format strings are.

**The smallest change that makes integer-only `printf` run today** is `musl-printf-nofp.patch` (in `vfprintf.c`
only, applied by `MUSL_PATCHES`): remove the `long double` member of `union arg`, make `pop_arg`'s `DBL`/`LDBL` cases
and the `%a %e %f %g %A %E %F %G` case a trap (`__builtin_trap()`, which the front lowers to the named fault
`unreachable`), and compile `fmt_fp` out. 4 hunks, 15 lines added. `musl-scanf-nofp.patch` does the same for
`vfscanf.c` (`__floatscan`). With them `%f` ends the run in the named fault `unreachable` (self-test `float.c`),
never prints something else. Effect: the step 2 -> 3 rows above (GCC 37: 5 -> 30 agree; c-testsuite 64: 0 -> 61).

**What SoftFloat behind the libcall interface should remove** (`__adddf3`-style calls for f32, f64 and x87 80-bit,
another job): with unpatched musl, every `printf`-family refusal here is floating point in the library: GCC 28 of the
37 and c-testsuite 62 of the 64 are `x86_fp80` at `vfprintf.c:627`. Three of those 28 GCC programs meet two more
barriers one after the other: past `vfprintf.c` they stop at `x86_fp80` in `vfscanf.c` (step 3), and past that at the
missing clock (step 5), which SoftFloat does not remove. So these 28 + 62 programs should get past the library's
floating point once it is lowered (with the patches, 25 of the GCC ones agreed at step 3; 61 of the c-testsuite ones agree and 1 faults for lack of
files), and the
two patches become unnecessary; then
`fmt_fp` and `__floatscan` actually run, which today nothing tests. The test programs' own floating point (GCC:
`gofast`, 129 in the whole corpus; c-testsuite: `00174`, `00175`, 6 in the whole corpus) should go too. What
SoftFloat-as-libcalls would not cover by itself, measured in musl's bitcode:

- **Constrained intrinsics.** musl builds with `-frounding-math`, so clang emits
  `llvm.experimental.constrained.fadd/fmul/fsub/fdiv/fcmp/fmuladd/sitofp/uitofp/fptosi/fptoui/fpext/fptrunc`, with
  dynamic rounding and strict exceptions, not plain `fadd`: 227 of 1,345 files (`vfprintf.c`: 11 `sitofp.f80.i32`, 9
  `fmul.f80`, 8 `fcmp.f80`, 7 `fpext.f80.f64`, 5 `fadd`, 5 `fsub`, ...; `floatscan.c` 33 `sitofp.f80`, 16 `fmul.f80`,
  6 `fmuladd.f80`, 4 `fdiv.f80`). The lowering must accept them (and either honour the rounding mode, which the
  `__adddf3` interface has no argument for, or document round-to-nearest; musl's generic `fenv.c` supports only
  `FE_TONEAREST`, so the second is consistent), or musl must be built without `-frounding-math`.
- **`long double` in memory and through unions.** 125 files load and store `x86_fp80` (`vfprintf.c` 40 loads and 24
  stores); `frexpl`, `__signbitl`, `__fpclassifyl`, `fabsl` read its bits through `union ldshape` (a 64-bit mantissa
  and a 16-bit sign-exponent in a 16-byte slot). The front must lay out `x86_fp80` as 10 bytes in 16 and allow integer
  loads of its parts, which is memory layout, not arithmetic.
- **`fneg`** (3 in `vfprintf.c`) is a sign-bit flip, not a libcall in compiler-rt's interface.
- **Inline assembly in libm**: 14 arch C files (x87/SSE `fsqrt`, `frndint`, `fprem`, `cvtsd2si`, ...) and 15 x87
  assembly files; `GENERIC=1` replaces all of them with C, which then needs `long double` arithmetic. `fenv.s` is
  replaced by generic C with one rounding mode and no exception flags; one more file uses `cpuid`-style asm in
  picolibc's `libm/machine/x86/fenv.c`.

## 4. Size and cost

musl, step 5 configuration, `LINK=internalize` (`python3 -I cost.py <bennett> <work dir>`; the executor timed again):

| program | functions | instructions | `.bn` bytes | of which `data` lines | steps | front s | front peak MiB | run s | run peak MiB |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| `puts("hello")` | 45 | 993 | 4,502,195 | 4,463,309 | 505 | 0.14 | 64 | 0.04 | 8 |
| `printf("hello %d\n", 42)` | 57 | 2,179 | 4,553,948 | 4,468,463 | 1,749 | 0.19 | 66 | 0.04 | 9 |
| `sprintf` | 47 | 2,056 | 86,284 | 6,620 | 3,368 | 0.15 | 55 | 0.00 | 5 |
| `printf-int` | 57 | 2,219 | 4,555,537 | 4,468,588 | 15,560 | 0.19 | 67 | 0.04 | 9 |
| `malloc` (mallocng) | 96 | 2,687 | 4,560,123 | 4,462,504 | 536,094 | 0.20 | 65 | 0.05 | 9 |
| `qsort` | 31 | 1,024 | 41,620 | 1,770 | 8,665 | 0.09 | 54 | 0.00 | 5 |

With `LINK=external`: `hello` 85 functions, 2,593 instructions, 1,770 steps, 0.16 s; `puts` 68 / 1,454 / 538;
`sprintf` 79 / 2,576 / 3,400. `hello` pulls 32 library members (`printf`, `vfprintf`, `fwrite`, `__stdout_write`,
`__stdio_write`, `__towrite`, `__lockfile`, `atexit`, `lite_malloc`, `wctomb`, `strerror`, ...); the system calls they
name are `writev`, `ioctl`, `lseek`, `close`, `brk`, `mmap`.

**Nothing like the earlier 63 MB object or the executor timeouts happens here.** The device code for `printf` is 2,179
instructions and about 85 KB of text; `bennett-front` takes 0.2 s and 66 MiB; the run takes 1,749 steps. The largest
cost over all 1,876 corpus programs: front at most 0.43 s and 84 MiB, no 900 s timeout, at most 9,663 device
instructions. The `.bn` size is dominated by something else: **the front writes zero-initialised data byte by byte
as `\x00` text**, 4 bytes of text per byte of memory, so the probe's 1 MiB arena and 64 KiB output buffer become
4.47 MB of `data` lines in every program that pulls them (`sprintf` and `qsort` do not). That is a property of the
`.bn` writer, not of the library; a `data` segment of zeros need not be written at all (`memory N` is zero already).

picolibc (section 5), same programs, `LINK=internalize`: `hello` 28 functions, 832 instructions, 299,020 bytes
(264,775 of them the output buffer), 760 steps, 0.07 s, 53 MiB; `puts` 25 / 319 / 405 steps; `sprintf` 9 / 642 / 606
steps. With `LINK=external`: `hello` 39 / 897 / 772 steps.

## 5. picolibc 1.8.12

Why picolibc rather than LLVM's libc: picolibc is C, builds the integer-only printf and scanf variants itself, has a
`FILE` that wraps put/get function pointers and newlib's small porting layer, and needs no C++ or a newer clang.
LLVM's libc is C++ (templates, its own build of the matching LLVM version; the baremetal configuration in LLVM 18 is
young), so it would test a C++ path through the front as much as a library.

**Compile.** meson's database for this configuration lists 1,076 sources; the library proper (`libc.a`, the
x86 machine library, `libstrcmp.a`) is 1,057: 1,047 C sources, all of which compile to bitcode as is, and 10 x86
assembly files. 7 of those (`memcpy`, `memset`, `memmove`, `memchr`, `memcmp`, `strchr`, `strlen`) have generic C
files in `libc/string`, which are compiled instead; 3 have none (`setjmp.S`, `interrupt.S`, `tcb.S`). No
system-call assembly, atomics or thread pointer: picolibc's porting layer is plain C functions
(`libos/dummyhost` has 16 stubs, which the probe runtime replaces). 373 files use floating point; one file has
non-empty inline assembly (`libm/machine/x86/fenv.c`).

**Run, substitution by substitution** (hand programs, `LINK=internalize`):

| step | substitution (cumulative) | agree | refused | fault | first refusal or fault |
|---|---|---:|---:|---:|---|
| 0 | picolibc as configured, probe porting layer | 4 | 6 | 0 | `the global @stdin, which has no definition`: an undefined *weak* reference (`&stdin != NULL`) |
| 1 | + `weak.py` (undefined weak to null) | — | — | — | `puts`: fault `bad_indirect` (measured on one program) |
| 2 | + `picolibc-bufio-abi.patch` | 9 | 0 | 1 | `fopen`: fault `unreachable` (no file system) |
| 3 | + `-Datomic-ungetc=false` | (self-test) | | | without it `scanf` is refused: `the LLVM instruction atomicrmw, at local-stdio.h:793` |
| 4 | + `picolibc-nofp-trap.patch` | 9 | 0 | 1 | |

Step 1's fault: on x86-64 picolibc assumes that a function taking an `int` file descriptor may be called through a
pointer typed with a `void *` first parameter (`BUFIO_ABI_MATCHES` in `local-stdio.h`), so `write(int, ...)` is
called as `ssize_t (*)(void *, const void *, size_t)`. Natively the ABI hides it; on the device the call's signature
does not match the target's and the run ends in `bad_indirect`. Named, not silent; the patch removes `__x86_64` from
that condition so that picolibc calls through the `int` member of its union.

Step 3's finding (self-test `float.c`, `sscanf("1.5", "%f", &f)`): picolibc's integer-only scanf stops at `%f` and
returns 0 matches; native prints 1. **The run succeeds with different output: a silent wrong result from the library's
configuration**, not from the front. Its integer-only printf prints the text `*float*` for `%f`, by design. The
patch makes both a trap (named fault `unreachable`); with it the self-test faults. Neither corpus list happened to
hit this, because their `%f` programs are refused at the test's own `double` first.

**The 37 and 64** (step 4 with an `unlink` stub, `LINK=external`): GCC 28 agree, 5 fault (at `open`; picolibc's
`tmpnam` needs no clock), 3 refused (`setjmp`, `longjmp`, `gofast`'s own `float`), 1 skipped; c-testsuite 61 agree, 2
refused (own floating point), 1 fault (`00187`, files). Exactly the musl outcome, program by program. Whole corpora,
`LINK=external`: see the table at the end of this section.

| outcome | GCC: baseline | picolibc | musl | c-testsuite: baseline | picolibc | musl |
|---|---:|---:|---:|---:|---:|---:|
| agrees (baseline: status only; probe: output and status) | 1,402 | **1,437** | 1,438 | 149 | **212** | 212 |
| refused by name | 172 | 138 | 138 | 70 | 7 | 7 |
| named fault (native passed) | 1 | 6 | 6 | 0 | 1 | 1 |
| **wrong** | 0 | **0** | 0 | 0 | **0** | 0 |
| crash | 1 | 1 | 0 | 0 | 0 | 0 |
| budget | 0 | 0 | 0 | 1 | 0 | 0 |
| skipped | 80 | 74 | 74 | 0 | 0 | 0 |

Program by program the two libraries give the same outcome everywhere except GCC `20051113-1`: with picolibc's
`malloc` and `calloc` in the module, LLVM 18's dead-store elimination crashes again (c-corpus cause 1), now reported
by name ("LLVM's optimiser crashed on this module", exit 2, counted as a crash as in c-corpus).

## 6. Where the boundary could sit

**The console effect from C.** The device program has `effect` (one `i8` to the console; the executor journals it,
`bennett run` prints `console: "..."`, and inside a coherent region it is the fault `effect_in_region`). The front at
`b875989` has no way to emit it: no marker names it (`front/src/Markers.h`), and every call to a function without a
definition is refused (`front/src/Emit.cpp`, `BodyWriter::call`). What it would take is small:
`front-effect.patch` (8 lines in `BodyWriter::call`) lowers a call of the declaration `void __bennett_effect(char)`
to `effect`. It must not be a marker, because every caller of a marker becomes a control function
(`front/src/Outline.cpp`, `controlFunctions`); as an opaque external call it already keeps LLVM from deleting or
reordering it, and a function that calls it does not only read memory, so it is never run coherently. A real
version would declare it in `front/include/bennett.h`, refuse other signatures by name as the markers are, and give
it a test.

Measured with that front (**front-effect** binary; nothing else differs from `b875989`) and the runtimes built
with `-DBENNETT_EFFECT`, so that every byte written to fd 1 is also an `effect`, and `CONSOLE=1`, so that the harness
compares `bennett run`'s `console:` line with native stdout directly: all 10 hand programs on both libraries give a
console equal to native stdout, except `fopen`, which faults before it prints (by design); this includes `exit`,
which ends in a fault, because the console survives the end of the run. c-testsuite in full (musl, `LINK=external`):
212 agree, and for all 212 the console equals native stdout and therefore the `.expected` file. Cost: one `effect`
per byte, e.g. `hello` 1,771 steps against 1,749.

**The three shapes, and what the probe measured for each.**

| shape | what the probe has | what it needs from the device |
|---|---|---|
| (a) one function taking a system-call number | musl: `syscall_arch.h` overlay -> `__bennett_syscall(n, a1..a6)`, a `switch` over `write`, `writev`, `read`, `ioctl`, `open`/`openat`, `close`, `lseek`, `brk`, `mmap`, `munmap`, `madvise`, `mprotect`, `mremap`, `rt_sigprocmask`, `rt_sigaction`, `gettid`, `getpid`, `set_tid_address`, `kill`/`tkill`/`tgkill`, `exit`/`exit_group`; anything else traps. Works for everything measured here; musl's code above it runs unchanged. | console (`effect`), an arena for `brk`/`mmap`, an end of the run with a status. The thread descriptor and startup are C in the runtime. The surface is musl's: 288 system calls are named in the library; the runtime implements 24, and the only unknown
one any measured program reached is `clock_gettime` (from `tmpnam`), which traps. |
| (b) the library's stdio back end | not measured separately; in musl it is `__stdio_write`/`__stdout_write` (both call `writev`/`ioctl` through (a)); in picolibc the `FILE`'s put function (`posixiob_stdout.c` puts it over `write`). Replacing one of these is a few lines but leaves `malloc` (`brk`/`mmap` or `sbrk`), `exit`, files and the rest on their own path. | console; plus everything (a) or (c) still needs for non-stdio calls |
| (c) the library's porting layer | picolibc: `write`, `read`, `_exit`, `sbrk`, `open`, `close`, `lseek`, `fstat`, `isatty`, `getpid`, `kill`, `unlink` in `picolibc-rt.c`, 12 functions; the same results as (a) on every program measured, except `20051113-1`, on which LLVM's optimiser crashes (reported by name) under picolibc and not under musl. | the same three: console, arena, end of run |

**What `exit` needs.** Today a run ends only by returning from the entry function or by a fault, so the probe
returns from `__bennett_entry` and must reproduce by hand what `exit` does on a return from `main`: atexit
handlers, then destructors, then the stdio flush (musl: `__funcs_on_exit`, `__libc_exit_fini`, `__stdio_exit`;
picolibc: `__call_exitprocs`, then the `.fini_array` destructor that flushes `stdout`). A program that calls
`exit(n)` or `abort()` from deep inside cannot return, which is why the probe ends those runs in a fault that only
says whether the output matched. A real design needs one device way to end the run with a status from anywhere
(an instruction, or an entry convention the executor honours), after which `exit` is the library's own code:
`atexit` is a table in the library, the flush is the library's, and constructors and destructors need the static
linker's `.init_array`/`.fini_array` (or `ctors.py`'s arrays) to be called by the startup code. Output written by
`write` before the end must survive the end, which `effect` already does (the console is not rolled back by a
fault).

## Findings

1. **A constructor that LLVM cannot evaluate at compile time is never run** (silent wrong result in `bennett-front`
   at `b875989`). `reduced/ctor-not-run.c`, 4 lines: a constructor copies a volatile into a global that `main`
   returns. Native exit status 5; device `result: 0`. The front lowers `@llvm.global_ctors` as data and nothing calls
   its entries (the same for `@llvm.global_dtors`). Expected: run them in the startup code, or refuse by name. Not
   repaired here. In the corpora only GCC `20001229-1` has a constructor, and it does nothing on x86-64, which is why
   the corpus did not see it. The probe works around it with `ctors.py`.
2. **One wrong result in the full GCC run, caused by the probe's link step** (`bcp-1`, section 2): internalising the
   program changes what `__builtin_constant_p` sees. Not a front bug. Reduced: `reduced/internalize-constant-p.c`.
3. **picolibc's integer-only scanf and printf give silently different output for floating-point conversions**
   (section 5). A library configuration, not a front bug; patched to trap.
4. **Undefined weak references and an ABI pun are refused or fault by name**, not silent: `the global @stdin, which
   has no definition` (picolibc), `bad_indirect` for `write(int)` through a `void *` pointer (picolibc on x86-64).
5. **Zero data costs 4 bytes of `.bn` text per byte** (section 4).
6. No crash of the front (other than LLVM's own, reported by name), no timeout, no executor limit, no cost
   explosion, in three full runs of both corpora (musl internalised and external, picolibc external), one console
   run of c-testsuite and the hand programs.

## Interpretation, and what would make these numbers misleading

- **The library is not the problem for integer code.** With three overlay headers, musl's generic C and one runtime
  file, plus the two no-floating-point stdio patches (`musl-printf-nofp.patch`, `musl-scanf-nofp.patch`), every
  integer program measured prints what native prints, except those that need files, a clock or `setjmp`/`longjmp`,
  and those that meet a front refusal or fault unrelated to the library (`pr92904`'s over-aligned variadic argument,
  `pr67037`, c-testsuite `00025`'s mismatched prototype, the return-address tests) or, under picolibc, LLVM's
  optimiser crash on `20051113-1`. picolibc needs one ABI patch, a no-floating-point trap patch and a configuration. What stops the rest is floating point (in the library's `printf` today, and in the
  programs themselves), `setjmp`/`longjmp`, and files.
- **`%f` is the line.** Integer-only `printf` today needs a patch; SoftFloat would remove it, and would also be the
  first thing to run `fmt_fp` and `__floatscan`, so the output comparison of this harness is the test that matters for
  it. The hand programs and the two lists contain no successful `%f` at all.
- **Coverage of the library is thin.** The hand programs pull 14 to 36 members of 1,345, single corpus programs up to
  78, and all GCC programs together 107 distinct members; `vfprintf`'s integer paths,
  `fwrite`, `mallocng`, `qsort`, `ctype`, `atexit` and `exit` run; locale, wide characters, `printf` positional
  arguments, `scanf` beyond a few cases, time, signals, threads do not. Agreement says nothing about them.
- **Agreement is (output, status) on one input**: stdin is empty, `argc` is 1, there are no files. A program whose
  behaviour depends on its environment is compared in that environment only. stderr is not compared.
- **A missing file system must fault, not fail.** With `FILES_ENOENT` (open fails with `ENOENT`), picolibc runs
  `printf-2`, `fprintf-2`, `user-printf` and c-testsuite `00187` to a successful end with output that differs from
  native: four "wrong" results that are the environment's, indistinguishable by the harness from a miscompilation.
  The default trap turns them into named faults.
- **The boundary is the probe's own code**, about 230 lines each; a bug there would show as a mismatch or a fault, not
  hide one, except that `brk`/`mmap`/`sbrk` semantics are simplified (no `munmap`, a fixed arena). `malloc` ran
  536,094 steps (musl) and 942,183 (picolibc) without a mismatch.
- **Internalisation inflates agreement**: of the 49 extra GCC agreements under `LINK=internalize`, 47 exist only
  because LLVM folded floating point after internalising (2 more folded a return address, 1 inlined a mismatched call), and it caused the one wrong result. The `LINK=external` columns are the
  conservative numbers.
- **One clang, one target.** Native and device share clang 18's front end and headers differ (glibc natively, musl or
  picolibc on the device), so a header-level difference shows up as a mismatch, not hidden; a bug shared by the two
  compilations (clang itself) is invisible.
- Not measured: LLVM's libc; picolibc's `FILE` put-function shape (b) separately; `atexit` beyond one handler;
  programs reading stdin; `.bn` size with the zero-data issue fixed; `front/test.sh` with the front-effect binary.

## Candidate designs for independent designers (not chosen)

1. **musl at the system-call boundary.** musl as it is, three overlay headers, generic C instead of assembly, a
   runtime with one `syscall(n, ...)` function implemented by device effects (console, arena, end of run) and
   faults for the rest; startup and exit in C. Expected, not measured here: the widest coverage of POSIX for little porting; the boundary is
   Linux's, wide (288 calls named, ~20 needed) but each is a `switch` case.
2. **picolibc at its porting layer.** picolibc built for the device (integer or, once SoftFloat lands, full stdio),
   the dozen newlib-style functions implemented by device effects; one ABI patch and the configuration upstream.
   Smaller device code on the programs measured (`hello` 832 instructions against 2,179; library size and porting
   effort were not compared); the porting layer is the documented one, a
   `FILE` with put/get pointers is the natural place for the console. Less of POSIX.
3. **Either library, with a device-level end of run and console in the front first.** Make `effect` reachable from C
   (as `front-effect.patch`) and give the device one way to end a run with a status, then put stdio's back end or the
   porting layer directly on them; the hash-and-fault oracle of this probe becomes unnecessary and the console is
   compared directly. This is a front and device change, which the other two can be built on.

## Files

- `musl-bc.sh`, `musl-overlay/{syscall,atomic,tp}/*.h`, `musl-printf-nofp.patch`, `musl-scanf-nofp.patch`,
  `musl-rt.c`: musl as bitcode and its probe runtime.
- `picolibc-bc.sh`, `picolibc-cmds.py`, `picolibc-bufio-abi.patch`, `picolibc-nofp-trap.patch`, `picolibc-rt.c`:
  the same for picolibc.
- `probe-one.sh`, `probe-all.sh <list or dir> <work dir> [jobs]` (`LIBDIR`, `LINK`, `BIN`, `FRONT_TIMEOUT`, ...):
  the harness. `resolve.py`, `ctors.py`, `weak.py`: the link steps. `hash.py`: the native side of the oracle.
- `classify.py`, `cost.py`, `summarize.py`: the tables. `granularity/`: the refusal-granularity programs.
- `programs/`: the hand programs. `selftest/`: the oracle's self-tests. `reduced/`: the reduced findings.
- `front-effect.patch`: the experimental front change of section 6 (not applied to `front/`).
- Work directories are under `.tmp/libc-probe/` (git-ignored): per-program outputs in `<work dir>/out/<name>/`.
