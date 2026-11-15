# rt/softfloat: floating point as library code

The device program has no floating-point values. `bennett-front` rewrites every `float`, `double`,
`long double`, `_Float16` and `__bf16` operation into a call of the compiler runtime-call interface (the libgcc names, `__adddf3`,
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

| `fmal` (SoftFloat has no extF80 fused multiply-add) | the exact 128-bit product of the significands and the addend's aligned in a 256-bit window, the bits shifted out jammed into the lowest one, added or subtracted, normalised, rounded once by `softfloat_roundPackToExtF80`; glibc's special cases (an infinite or NaN operand, a zero factor or addend) computed as glibc computes them, with SoftFloat's extF80 operations, which match x87's | none, NaN bits included, against glibc's `fmal`: the cross product of the directed values (830,584 triples) and 10^8 random triples (`float_runtime_exact 100000000 3 0 fma`, 52 s), steered to cancellation (the addend near or minus the product), subnormal results and overflow; only encodings x87 rejects differ |

| `_Float16`: `__extendhfsf2`, `__extendhfxf2`, `__truncsfhf2`, `__truncdfhf2`, `__truncxfhf2` | SoftFloat's `f16` conversions, one rounding each | none, NaN bits included: every 16-bit input, every 32-bit `float` input, 10^7 random `double` and `long double` operands (steered to `_Float16`'s and `float`'s subnormal ranges) |
| `__fixhfti`, `__fixunshfti`, `__floattihf`, `__floatuntihf` | as libgcc's soft-fp routines: saturating (an infinity or NaN gives the largest or least integer by its sign; unsigned, a negative value 0), and an integer of 2^64 or more is an infinity | none: every 16-bit input; 10^7 random integers |
| `__bf16`: `__truncsfbf2`, `__truncdfbf2`, `__truncxfbf2` | from `float` to nearest even on the bits, a NaN quiet with the top of its payload; from `double` and `long double` rounded to `float` to odd (an inexact `float` made odd towards the exact value), then to `__bf16`: one correct rounding, since `float` keeps at least two more bits everywhere | none, NaN bits included: every 32-bit `float` input, 10^7 random `double` and `long double` operands, against libgcc (`__truncxfbf2` called directly: clang 18's back end cannot compile `long double` to `__bf16`) |

The whole set, measured on 2026-11-15 with `float_runtime_exact 10000000 3 1` (6 min, 3 threads): 0 mismatches
on all 26 routines; only long double encodings x87 rejects differ. `front/tests/tests/softfloat.rs` repeats
it with 2·10^4 random operands per operation (and every 16-bit input, and the directed triples of `fmal`) in
under a second.

### _Float16 and __bf16 on the device

The front computes them as LLVM's x86-64 back end does without half-precision hardware (measured from its
output): every operation in `float`, the result rounded back to the type after each one, `-x` and `fabs`
included (so a signalling NaN comes out quiet); `copysign` and the class tests are bit operations. `__bf16`
to `float` is the top half of the bits. A conversion from an integer to `__bf16` goes through `float` and so
is rounded twice, as natively; `_Float16` has libgcc's 128-bit routines, `__bf16` the `float` ones. `powi` on
them is refused by name (the back end calls the C library's `powf`). Between `_Float16` and `__bf16` clang 18
writes IR that LLVM rejects (neither type is the wider), so a program converts through `float`.
`front/tests/c/float_half.c` checks 5,320 directed results of both types on the device against the native
program at `-O0` and `-O2`. Integers from 65 to 128 bits wide go through the 128-bit routine of their signedness and are
truncated, as LLVM's x86-64 back end does (a `_BitInt(100)` included); a 128-bit integer crosses the interface
as C passes it on x86-64 (two 64-bit arguments, a pair of 64-bit results), as clang writes it in the runtime's
IR. Wider integers are refused by name.

## Cost on the device

Device steps of one operation, after the front's optimiser, on normal / subnormal / NaN operands (the steps of
moving the operands taken off), and the routine's size in device instructions including the helpers it calls.
The rows for long double `fma` and below the table for the routines added since were measured on 2026-11-15
by lowering a function of the operands' bits at clang `-O2` and subtracting a function that only moves them
(the same measurement gives 412 instructions and 63 / 18 / 39 steps for the f64 add).

| operation | f32 size | f32 steps | f64 size | f64 steps | f80 size | f80 steps |
|---|---:|---|---:|---|---:|---|
| add | 390 | 70 / 29 / 47 | 431 | 67 / 22 / 43 | 480 | 75 / 101 / 57 |
| sub | 390 | 112 / 87 / 45 | 431 | 115 / 85 / 43 | | |
| mul | 199 | 59 / 145 / 33 | 218 | 55 / 167 / 29 | 249 | 72 / 164 / 45 |
| div | 198 | 56 / 138 / 33 | 252 | 73 / 178 / 29 | 363 | 139 / 219 / 45 |
| sqrt | 162 | 92 / 142 / 23 | 184 | 114 / 161 / 22 | 251 | 131 / 184 / 21 |
| fma | 439 | 108 / 228 / 61 | 476 | 106 / 262 / 50 | 1198 | 299 / 261 / 119 |
| `<` | | | 43 | 30 / 37 / 9 | 67 | 48 / 51 / 20 |
| `==` | | | 22 | 22 / 22 / 9 | | |

Added on 2026-11-15, by the measurement above (instructions; steps on normal / subnormal / NaN operands, or
three integers): `double` to `__int128` 757, 339 / 290 / 209 and to `unsigned __int128` 713, 328 / 279 / 198
(libgcc's two-word algorithm: two multiplications, a subtraction, three conversions); `__int128` to `double`
118, 118 steps; `long double` to `__int128` 3173, 257 / 265 / 142 (the extF80 operations it uses return
structures and are inlined); `__int128` to `long double` 125, 116–125 steps. `_Float16`: add 536, 139 / 235 /
96; mul 360, 134 / 190 / 85; `<` 122, 67 / 158 / 45; from `float` 84, 46 / 60 / 20; to `float` 79, 20 / 62 /
19. `__bf16`: add 393, 77 / 39 / 54; mul 217, 72 / 127 / 43; from `float` 17, 11–14 steps; from `double` 217,
113 / 168 / 57; to `float` 2 and 2.

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
the front re-infers memory effects afterwards. A routine that returns an aggregate (SoftFloat's
`extFloat80_t`, a 128-bit integer as C returns it on x86-64) is inlined by the front wherever it is called,
since a device function returns one integer and an aggregate would come back through memory. So no routine
writes memory, and floating-point code runs in control code, under a qubit branch, and inside a coherent
region alike (`fmal` and the 128-bit conversions were refused under qubit control at `-O2` before that). C can observe the flags and the
rounding mode only through `<fenv.h>`, which belongs to the C library; `#pragma STDC FENV_ACCESS` (constrained
intrinsics) is refused by name.
