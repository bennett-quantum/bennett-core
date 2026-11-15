# rt/softfloat: floating point as library code

The device program has no floating-point values. `bennett-front` rewrites every `float`, `double` and
`long double` operation into a call of the compiler runtime-call interface (the libgcc names, `__adddf3`,
`__ltsf2`, `__fixdfsi`, `__mulxf3`, ..., and for the rest the C library names of LLVM's RuntimeLibcalls.def,
`sqrt`, `fma`, `floor`, `fmin`, ...), and links the routines from a runtime bitcode (`front/src/SoftFloat.h`).
This directory is the default runtime: Berkeley SoftFloat 3e, vendored unmodified (`VENDOR.md`), with the
8086-SSE specialisation, under the interface in `libgcc.c`. CMake builds it into `target/front/float-runtime.bc`;
`bennett-front --float-runtime file.bc` takes any bitcode that defines the same names. Decided by the
maintainer on 2026-11-15 (`docs/DESIGN.md`, "Floating point").

## Bit for bit against this machine's hardware

Measured on 2026-11-15 on x86-64 (Zen 2, glibc 2.39, clang 18.1.3), SoftFloat compiled natively as the runtime
is, against SSE for `float` and `double` and x87 for `long double` (reference compiled `-O2 -ffp-contract=off`,
operands through `volatile`). Directed values: the cross product of ±0, smallest and largest subnormals,
smallest normal, ±1, 1±ulp, largest finite, ±inf, quiet and signalling NaNs with several payloads, ties,
cancellation, overflow and underflow (9,604 pairs for f64, 15,876 for extF80, 941,192 triples for fma). Random:
10^7 tuples per operation and format from mixed generators. Exhaustive: every 32-bit input of each unary `float`
operation and of the 32-bit integer conversions.

| format: operations | mismatches |
|---|---|
| f32, f64: add, sub, mul, div, sqrt, neg, fabs, trunc, floor, ceil, round, roundeven, rint, lrint, lround, all 14 fcmp predicates, conversions among f32/f64/f80 and to and from i32/i64/u32/u64 | none, NaN bits included |
| f32, f64: fma | which NaN, only: for `fma(0, inf, NaN)` x86 returns the NaN operand, SoftFloat the default NaN; of several NaN operands x86's choice depends on the instruction form the compiler picks |
| extF80, canonical encodings: the same operations | none, NaN bits included (SoftFloat follows x87: the larger significand wins) |
| extF80: pseudo-denormals, unnormals, pseudo-infinities, pseudo-NaNs | differ (x87 reads or rejects them otherwise); no arithmetic produces them, only `memcpy` |
| out-of-range conversions to integers (undefined in C) | SoftFloat saturates; `libgcc.c` gives what clang's x86-64 code gives instead |

`front/tests/tests/softfloat.rs` repeats the directed set and a fixed random sample in about 1.5 s and checks
that these classes are the only ones. The promise that follows (`docs/DESIGN.md`): every result that is not a
NaN is bit for bit the native one; a NaN result is a NaN. Which NaN, when several operands are NaNs, is
unspecified, as in LLVM (whose optimiser commutes operands) and on the hardware.

## The runtime's own routines, against the native program

Some routines of `libgcc.c` are more than a call of SoftFloat. They are checked against what the native program
calls for the same C operation (clang 18 on x86-64 links GCC 13's libgcc and glibc), by
`front/tests/c/float_runtime_exact.c`: the device's own `float-runtime.bc` compiled natively, every symbol
prefixed `rt_`, so that `rt___fixdfti` is the runtime's and `__fixdfti` libgcc's. Measured on 2026-11-15 on the
machine above with `float_runtime_exact 10000000 3 1`: directed values, 10^7 random operands per operation, and
every 32-bit input of each conversion from `float`.

| routines | how | mismatches |
|---|---|---|
| `__floattisf`, `__floattidf`, `__floattixf` and the unsigned `__floatunti*` | the magnitude's top 64 bits, the rest jammed into the lowest bit (extF80 keeps it as its extra word), rounded by SoftFloat's `softfloat_roundPackTo*` | none (ties and sticky bits at each format's precision, every power of two ±2) |
| `__fixsfti`, `__fixdfti`, `__fixxfti` and the unsigned `__fixunss*ti` | libgcc2.c's algorithm, which libgcc's routines are on x86-64, step for step over SoftFloat: the high word from x / 2^64, the low word from what is left, each through GCC's 64-bit unsigned conversion; negative values through their negation. Out of range and NaN (undefined in C) give what libgcc gives, e.g. 2^127 + 2^63 for a NaN | none, out of range included; long double encodings x87 rejects differ, as above |

`front/tests/tests/softfloat.rs` repeats this with 2·10^4 random operands per operation in well under a
second. Integers from 65 to 128 bits wide go through the 128-bit routine of their signedness and are
truncated, as LLVM's x86-64 back end does (a `_BitInt(100)` included); a 128-bit integer crosses the interface
as C passes it on x86-64 (two 64-bit arguments, a pair of 64-bit results), as clang writes it in the runtime's
IR. Wider integers are refused by name.

## Cost on the device

Device steps of one operation, after the front's optimiser, on normal / subnormal / NaN operands (the steps of
moving the operands taken off), and the routine's size in device instructions including the helpers it calls.

| operation | f32 size | f32 steps | f64 size | f64 steps | f80 size | f80 steps |
|---|---:|---|---:|---|---:|---|
| add | 390 | 70 / 29 / 47 | 431 | 67 / 22 / 43 | 480 | 75 / 101 / 57 |
| sub | 390 | 112 / 87 / 45 | 431 | 115 / 85 / 43 | | |
| mul | 199 | 59 / 145 / 33 | 218 | 55 / 167 / 29 | 249 | 72 / 164 / 45 |
| div | 198 | 56 / 138 / 33 | 252 | 73 / 178 / 29 | 363 | 139 / 219 / 45 |
| sqrt | 162 | 92 / 142 / 23 | 184 | 114 / 161 / 22 | 251 | 131 / 184 / 21 |
| fma | 439 | 108 / 228 / 61 | 476 | 106 / 262 / 50 | refused | |
| `<` | | | 43 | 30 / 37 / 9 | 67 | 48 / 51 / 20 |
| `==` | | | 22 | 22 / 22 / 9 | | |

Conversions (f64): to i32/i64/u64 21–27 instructions, 10–21 steps; i32 to f64 53 and 53; i64/u64 to f64 131/140,
67–82 steps; f32 to f64 77, 13–60 steps; f64 to f32 85, 22–45 steps; f64 to f80 80, 21–68 steps; f80 to f64 95,
24–61 steps. A program with one `double` addition grows from 6 to 418 device instructions. The routines branch
(SoftFloat is ordinary C); under qubit control each branch configuration runs its own path.

## Global state

SoftFloat keeps the rounding mode, the tininess rule, the 80-bit rounding precision and the exception flags in
globals. The first three are never written; once the runtime is internal to the program LLVM folds their loads
to the initial values (round to nearest even, tininess after rounding as x86 detects it, 80-bit precision as
Linux sets x87). The flags are only ever or-ed into: `libgcc.c` supplies a `softfloat_raiseFlags` that does
nothing, SoftFloat's direct writes to the flags are deleted by the optimiser because nothing reads them, and
the front re-infers memory effects afterwards. So no routine writes memory, and floating-point code runs in
control code, under a qubit branch, and inside a coherent region alike. C can observe the flags and the
rounding mode only through `<fenv.h>`, which belongs to the C library; `#pragma STDC FENV_ACCESS` (constrained
intrinsics) is refused by name.
