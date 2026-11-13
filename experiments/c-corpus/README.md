# C corpus: does the C front end handle ordinary C correctly, or refuse it by name?

The question: three testers found silent miscompilations of ordinary C that the 15-program hand corpus missed. The
design rule is that `bennett-front` either handles a program correctly, refuses it by name, or the device program ends
in a named fault; never a silent wrong result, a crash or a hang. How true is that on a few thousand real self-checking
test programs, and why are programs refused?

Measured on 2026-11-13 at commit `4b65247` (tree clean when the binaries were copied). `bennett-front` and
`bennett` were built once (`cargo build --release`, `front/test.sh`, all tests passing), then copied to
`.tmp/c-corpus/bin/`, and only those copies were used. File and line references below are at `4b65247`.

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

## Results

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

### Refusal reasons: the work list (GCC, README pipeline, 334 refusals)

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

## Every distinct crash and fault cause

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

## Measured facts

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
