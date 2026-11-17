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
| extF80: pseudo-denormals, unnormals, pseudo-infinities, pseudo-NaNs | SoftFloat's own operations differ (x87 reads or rejects them otherwise); no arithmetic produces them, only `memcpy`. The runtime reads them as x87 does ("Noncanonical long double operands") |
| out-of-range conversions to integers (undefined in C) | SoftFloat saturates; `libgcc.c` gives what clang's x86-64 code gives instead |

`front/tests/tests/softfloat.rs` repeats the directed set and a fixed random sample in about 1.5 s and checks
that these classes are the only ones. The promise that follows (`docs/DESIGN.md`): every result that is not a
NaN is bit for bit the native one; a NaN result is a NaN. Which NaN, when several operands are NaNs, is
unspecified, as in LLVM (whose optimiser commutes operands) and on the hardware.

## The runtime's own routines, against the native program

Some routines of `libgcc.c` are more than a call of SoftFloat. They are checked against what the native program
calls for the same C operation (clang 18 on x86-64 links GCC 13's libgcc and glibc), by
`front/tests/c/float_runtime_exact.c`: the device's own `float-runtime.bc` compiled natively, every symbol
prefixed `rt_`, so that `rt___fixdfti` is the runtime's and `__fixdfti` libgcc's. Where clang 18's back end
cannot compile the operation (`long double` to `__bf16`), the reference is the libgcc routine a GCC build
calls, called directly.

| routines | how | mismatches |
|---|---|---|
| `__floattisf`, `__floattidf`, `__floattixf`, `__floatuntisf`, `__floatuntidf`, `__floatuntixf` | the magnitude's top 64 bits, the rest jammed into the lowest bit (extF80 keeps it as its extra word), rounded by SoftFloat's `softfloat_roundPackTo*` | none (ties and sticky bits at each format's precision, every power of two ±2, random) |
| `__fixsfti`, `__fixdfti`, `__fixxfti`, `__fixunssfti`, `__fixunsdfti`, `__fixunsxfti` | libgcc2.c's algorithm, which libgcc's routines are on x86-64, step for step over SoftFloat: the high word from x / 2^64, the low word from what is left, each through GCC's 64-bit unsigned conversion; negative values through their negation. Out of range and NaN (undefined in C) give what libgcc gives, e.g. 2^127 + 2^63 for a NaN | none, out of range included; every 32-bit `float` input |
| `fmal` (SoftFloat has no extF80 fused multiply-add) | the exact 128-bit product of the significands and the addend's aligned in a 256-bit window, the bits shifted out jammed into the lowest one, added or subtracted, normalised, rounded once by `softfloat_roundPackToExtF80`; glibc's special cases (an infinite or NaN operand, a zero factor or addend) computed as glibc computes them, with SoftFloat's extF80 operations, which match x87's | none, NaN bits included, against glibc's `fmal`: the cross product of the directed values (830,584 triples) and 10^8 random triples (`float_runtime_exact 100000000 3 0 fma`, 52 s), steered to cancellation (the addend near or minus the product), subnormal results and overflow |
| `__extendhfsf2`, `__extendhfxf2`, `__truncsfhf2`, `__truncdfhf2`, `__truncxfhf2` (`_Float16`) | SoftFloat's `f16` conversions, one rounding each | none, NaN bits included: every 16-bit input, every 32-bit `float` input, random `double` and `long double` operands steered to `_Float16`'s and `float`'s subnormal ranges |
| `__fixhfti`, `__fixunshfti`, `__floattihf`, `__floatuntihf` | as libgcc's soft-fp routines: saturating (an infinity or NaN gives the largest or least integer by its sign; unsigned, a negative value 0); an integer of 2^64 or more is an infinity | none: every 16-bit input |
| `__truncsfbf2`, `__truncdfbf2`, `__truncxfbf2` (`__bf16`) | from `float` to nearest even on the bits, a NaN quiet with the top of its payload; from `double` and `long double` rounded to `float` to odd (an inexact `float` made odd towards the exact value), then to `__bf16`: one correct rounding, since `float` keeps at least two more bits everywhere | none, NaN bits included: every 32-bit `float` input |

The whole set, measured on 2026-11-15 with `float_runtime_exact 10000000 3 1` (7.5 min, 3 threads): directed
values, 10^7 random operands per operation and every 32-bit input of each conversion from `float`: 0 mismatches
on all 25 operations, NaN bits included; only long double encodings x87 rejects (unnormals, pseudo-denormals,
pseudo-infinities, pseudo-NaNs, which no arithmetic produces) differ. Since the runtime reads those as x87 does
(next section), they differ only where the native side is software reading them at face value: glibc's `fmal`
and libgcc's truncations to `_Float16` and `__bf16`. `front/tests/tests/softfloat.rs` repeats
it with 2·10^4 random operands per operation, every 16-bit input and the directed triples of `fmal`, in under a
second.

Integers from 65 to 128 bits wide go through the 128-bit routine of their signedness and are truncated, as
LLVM's x86-64 back end does (a `_BitInt(100)` included); a 128-bit integer crosses the interface as C passes
it on x86-64 (two 64-bit arguments, a pair of 64-bit results), as clang writes it in the runtime's IR. Wider
integers are refused by name.

### _Float16 and __bf16 on the device

The front computes them as LLVM's x86-64 back end does without half-precision hardware (read from its
output): every operation in `float`, the result rounded back to the type after each one, `-x` and `fabs`
included (so a signalling NaN comes out quiet); `copysign` and the class tests are bit operations. `__bf16`
to `float` is the top half of the bits. A conversion from an integer to `__bf16` goes through `float` and so
is rounded twice, as natively; `_Float16` has libgcc's 128-bit routines, `__bf16` the `float` ones. `powi` on
them is refused by name (the back end calls the C library's `powf`). Between `_Float16` and `__bf16` clang 18
writes IR that LLVM rejects (neither type is the wider), so a program converts through `float`.
`front/tests/c/float_half.c` checks 5,320 directed results of both types on the device against the native
program at `-O0` and `-O2`.

## Noncanonical long double operands (2026-11-16)

x87 checks every operand's encoding before it computes; SoftFloat takes encodings at face value. The
encodings no arithmetic produces, only a write of the bytes, measured on this machine (Zen 2) against x87
instructions and glibc's x87 `fminl`/`fmaxl`:

- a **pseudo-denormal** (exponent 0, integer bit set) is the denormal of the same value. SoftFloat computes
  its products and quotients right but orders it below the smallest normal, and some sums differ;
- an **unsupported encoding** (an exponent other than 0 with the integer bit clear: an unnormal, an unnormal
  zero, a pseudo-infinity, a pseudo-NaN) is an invalid operand: the result is the indefinite (sign set,
  exponent all ones, significand `0xC000000000000000`), also beside a NaN operand, comparisons are
  unordered, and a conversion converts the indefinite. SoftFloat computes with it as with a number or a NaN.

`libgcc.c` reads every long double operand as x87 does (`x80_operand`): one test finds the noncanonical
encodings (exactly those whose integer bit equals "the exponent is 0"), a pseudo-denormal becomes the
normal number with exponent 1 and the same significand, an unsupported encoding becomes the indefinite and
the invalid flag is raised, and a two-operand operation with an unsupported operand gives the indefinite.
glibc's `fminl`/`fmaxl` are followed instruction by instruction (an unordered pair gives the other operand
when exactly one is unordered with itself and its significand's bit 62 is set, otherwise the sum).

`front/tests/c/float_noncanonical_x87.c` (run by `front/tests/tests/softfloat.rs`) puts 14 noncanonical
encodings, every class, against 14 canonical partners on both sides of each operation: arithmetic, the six
comparisons and `unordered`, conversions to `float`, `double`, every integer width up to 128 bits, `sqrtl`,
`floorl`, `ceill`, `truncl`, `rintl`, `nearbyintl`, `fminl`, `fmaxl`, `powi`. Every result bit agrees with
the native program, NaNs included; with the runtime before this check, 2,198 of those results differed.
Three routines are software natively and read the encodings at face value, so they still differ, by design: glibc's
`fmal` and libgcc's truncations to `_Float16` and `__bf16` (there is no x87 instruction for them).
`float_runtime_exact.c` now expects noncanonical differences only for those. glibc's `roundl` and
`roundevenl` are software too; since 2026-11-17 the runtime follows their bit operations (below).

Cost, device steps of one operation on normal operands at `-O2` (whole probe: operand loads and the
result's bits included): add 89 to 101, mul 77 to 89, div 144 to 156, `<` 52 to 62, `==` 28 to 38, to
`double` 42 to 47, to `long long` 20 to 24, `sqrtl` 160 to 165, `floorl` 47 to 51: 4 to 5 steps per
operand. A first version that LLVM made branch-free with selects cost 10 per operand. On the existing
long-double tests, every result unchanged: `float_long_double.c` (9,268 results) 1,927,170 to 2,119,358 steps
at `-O0` (+10.0%) and 1,649,392 to 1,842,776 at `-O2` (+11.7%); `float_long_double_programs.c` 75,851 to
84,573 (+11.5%) and 73,166 to 81,703 (+11.7%).

## Independent conformance pass

`front/tests/tests/p0_independent.rs` independently tests the noncanonical operand fix at clang `-O0`
and `-O2`. Its reference is native x86-64 C (volatile raw operands, no copied normalization algorithm).
The large matrix compiles the actual device runtime bitcode natively with renamed symbols, as the existing
runtime tests do; the smaller pipeline tests go through clang, the front and the executor. Result comparisons
are exact, including NaN bits and excluding the six ABI padding bytes.

- `p0_x87_matrix.c`: 354 inputs, a Cartesian sign/exponent/significand set plus fixed random raw and
  canonical encodings. All classes meet every other class on both sides. **2,388,054 checks per level**:
  add/sub/mul/div, their results fed to addition, all six comparisons and unordered, `fminl`/`fmaxl` and
  their results fed to arithmetic, sqrt/floor/ceil/trunc/rint/nearbyint, and conversions to/from float,
  double and signed/unsigned 32/64/128-bit integers, and roundeven. Canonical cases also cover round/fma and
  half/bfloat truncation. **3,894 powers** per level (exponents -17,-4,-3,-2,-1,0,1,2,3,4,17 on every
  input, noncanonical ones at exponent 1 included since that defect was fixed), **120 integer boundary
  conversions**, and
  **139,264 floating-source conversions** (every half/bfloat encoding; double samples in every exponent).
- `p0_x87_pipeline.c`: **4,640 results per level**, including negation/fabs/copysign, narrow integers,
  lrint/llrint, all ordered/unordered comparison combinations, classification masks and arithmetic after
  min/max/add. Every classification bit is compared at both levels (until the fix below, O0 excluded the
  two witnessed predicate bits in fourteen noncanonical classification cases). Ten noncanonical half-conversion cases are
  excluded by explicit operation/operand identity and have a minimal witness. Twelve undefined NaN/unsupported
  to-u64 cases at O2 are measured separately: native gives 2^63, the device gives 0 (the native O0 result).
  This is recorded target behavior, not counted as conformance or as a defined-C defect.
- Generated `_BitInt` probe: **1,024 native/device results per level** across every supported integer
  width 1..128, both signednesses (signed starts at 2), in both directions and with result reuse.
  Floating-to-integer values here are representable after truncation. One MiB of stack accommodates
  the generated dispatch function's SSA frame.

All passing comparisons agree. The new ignored witnesses below were explicitly run and failed on the
tested implementation; their native witnesses return 0. `witness()` reports both optimization levels
before asserting, so a failure at the first level does not hide the second.

| Witness in `p0_independent.rs` | Difference | Severity |
|---|---|---|
| `powi_one_preserves_a_pseudo_denormal_encoding` | `powi(x,1)`: native preserves exponent 0, runtime changed it to 1 | Fixed (below); runs by default |
| `powi_one_preserves_an_unsupported_encoding` | Native returns the unsupported argument unchanged; runtime replaced it with indefinite | Fixed (below); runs by default |
| `roundeven_of_an_unnormal_matches_native_software` | For exponent 1/significand 1, glibc returns +0, runtime NaN; adding 1 preserves the difference | Fixed (below); runs by default |
| `a_pseudo_denormal_is_finite` | Native true, device false at O0; O2 agrees | Fixed (below); runs by default |
| `an_unsupported_encoding_is_nan` | Native true, device false at O0; O2 agrees | Fixed (below); runs by default |

The expected `powi` bits follow native libgcc's exponent-1 path, which returns without arithmetic.
**Fixed:** `__powixf2` read its argument as x87 does at entry, so `powi(x, 1)` returned the canonical
reading. libgcc2.c starts from `y = n % 2 ? x : 1` and for `m == 1` returns `x` without an operation; on
every other path `x` meets a multiplication or the division, which read it as x87 does, so the runtime now
returns `x` for `m == 1` and keeps the single reading at entry otherwise. Both witnesses failed before the
fix and pass after it; the matrix's powers no longer skip noncanonical inputs at exponent 1 (3,894 agree at
each level, 470 differ with the early return moved to `m == 2`). Cost, one comparison per call:
`float_long_double.c` 2,119,358 to 2,119,622 steps at `-O0` and 1,842,776 to 1,843,040 at `-O2`;
`float_long_double_programs.c` unchanged (84,573 and 81,703).
`roundevenl` examines the encoding in software, as `roundl` does. **Fixed:** the runtime's `roundevenl`
read its operand as x87 does and rounded with SoftFloat; glibc's (ldbl-96) rounds the significand as an
integer at the binary point the exponent places and never reads the integer bit. The runtime now does the
same for every noncanonical encoding below the largest exponent (`roundeven_face_value` in `libgcc.c`,
glibc's quirks included: an unnormal at exponent 0x3FFE whose low 32 significand bits are not 0 rounds to
1, an unnormal can round to an unnormal zero), and keeps SoftFloat for the rest. A new matrix mode,
`rounding`, puts glibc against the runtime on all 65,536 sign/exponent words with 26 significands each
(22 fixed at every binary-point place, 4 random): **1,703,936 agree at each level**, and the main matrix
now meets `roundevenl` on all 354 inputs (2,388,177 checks). Dropping the low-word half of the 0x3FFE test
gives 26 differences. Cost: 44 steps on `float_long_double.c` at each level (2,119,666 and 1,843,084),
`float_long_double_programs.c` unchanged. Native aggregate classification uses
x87 comparisons at O0 where the front's bit classification has different answers. **Fixed:** at `-O0`
clang emits `llvm.is.fpclass`, which LLVM's x86-64 back end expands mask by mask
(`TargetLowering::expandIS_FPCLASS`): for long double, "zero or subnormal" is "the exponent is 0" (a
pseudo-denormal is finite), "NaN" outside `strictfp` is an x87 comparison (an unsupported encoding is
unordered) and inside it a bit test that also takes in pseudo-denormals, and the masks it inverts first
(LLVM's `invertFPClassTestIfSimpler`) follow suit. The front tested class by class with "no class" for
every noncanonical encoding. It now follows the expansion (`front/src/FpClass.h`, called with LLVM's own
inversion), and `front/tests/tests/fp_class.rs` compares all 1,024 masks on 48 encodings (every class, both
signs) with and without `strictfp` against clang's native code: 98,304 bits, 13,932 of them different with
the old front, none now. Its encodings are integers in memory: LLVM's constant folder rewrites a
noncanonical `x86_fp80` constant (a pseudo-denormal constant comes out with exponent 1), which hid the
pseudo-denormal cases in a first version of the probe. Cost on `float_long_double.c`: 2,119,666 to
2,119,578 steps at `-O0`, 1,843,084 to 1,843,216 at `-O2`; `float_long_double_programs.c` unchanged.
The already documented `roundl`, half and bfloat software differences each now have a minimal ignored
witness too (`round_of_an_unnormal_matches_native_software`, `half_conversion_of_an_unnormal_matches_native_software`,
`bfloat_conversion_of_an_unnormal_matches_native_software`): exponent 1/significand 1 becomes zero
natively and NaN on the device, at both levels. **Fixed for `roundl`** (beyond the pass's list, the same
cause as `roundevenl`): glibc's ldbl-96 `roundl` works on the two 32-bit halves of the significand and never
reads the integer bit; the runtime follows it on every noncanonical encoding below the largest exponent
(`round_face_value`, glibc's quirks included: at exponent 0x3FFE the result is 1 whatever the significand,
and rounding in the high half sets the integer bit, in the low half leaves it). The matrix's `rounding`
mode now checks `roundl` too (3,407,872 checks per level with `roundevenl`, all agree), the main matrix meets
it on all inputs (2,388,300 checks), and `float_noncanonical_x87.c` expects it to agree. Mutants caught by
the sweep: no integer bit set (799 differences), no carry into the high half (204), no 0x3FFE case (32).
glibc's carries into the exponent cannot happen without the integer bit and are not written (in
`roundevenl`'s too, where an earlier version kept one). Cost: `float_long_double.c` 2,119,646 steps at
`-O0` and 1,843,304 at `-O2` (+68 and +88 against the previous step, measured on a base two machine commits
older), `float_long_double_programs.c` unchanged. `fmal`'s previously documented noncanonical differences
remain covered by the existing runtime sweep; this pass's canonical fma probes agree.

The same pass's `crates/ir/tests/p0_angles.rs` checks all three axes in four register/memory/control
forms: 16 nonfinite/overflow spellings, 384 additional generated overflows naming the literal, 4,105 finite
spellings per axis (every binary64 exponent, boundary rounding, signed zero, subnormals and underflow),
and 256 generated readable modules with varying integer widths, function order, escapes and deliberately
invalid validation properties. Reading/writing preserves angle bits and module equality. No reader defect
was found. `front/tests/tests/p0_invalid_input.rs` checks textual syntax failures, malformed signatures,
eight truncations of real bitcode, PHI/dominance verifier failures and valid input execution. Three ignored
front witnesses failed: ordinary parse failures omitted the named invalid-input message (status 2 already
right, low severity; fixed, the front names it after LLVM's diagnostic); empty input emits an empty module
and exits 0 (the witness asked for a refusal, but an empty file is LLVM's empty module, which `llvm-as` and
the verifier accept, and no document asks the front to refuse a module without `main`: the witness now
expects acceptance); malformed subprogram metadata in a
44-byte bitcode file segfaulted in LLVM's reader instead of returning 2 (medium severity, compiler crash;
fixed: a signal while reading is caught and the input named, status 2).
The existing over-aligned-byval test continues to cover debug-reader fatal errors and ordinary verification.

Reproduce with `front/test.sh --test p0_independent --test p0_invalid_input -- --nocapture` and
`cargo test -p bennett-ir --test p0_angles`. Add `--ignored` after the final `--` to run the failing
witnesses. Default checks leave those witnesses ignored.

Not measured: exhaustive 80-bit inputs or all binary64 angles, every integer value, all fma triples,
every LLVM bitstream corruption, nondefault rounding/flags/traps, other native C libraries or CPUs,
coherent superpositions or reversal of these probes. The pass changed no production code; the fixes made
since are named above.

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

## The long-double step increase, measured (2026-11-16)

This is the remaining debt after the aggregate-inlining fix, not a floating-point accuracy change.
Identical musl-linked input bitcode, 1 MiB stack, clang/LLVM 18.1.3 and the same release executor were
used with freshly rebuilt historical fronts and their own floating-point runtimes. The historical
sources must get fresh modification times when reusing a CMake build directory: `git archive` preserves
old times and otherwise a nominal rebuild can reuse a newer object file. Successful executed-site
counters were checked against uninstrumented totals. All completed cases below return 0 and match native
stdout; no wrong result was found.

**Bisection:** `72807c5` and `e8a9a95` (immediately before `a9ce585`) give 3,863 / 4,145 steps for
`960215-1` / `regstack-1`; the inlining-era midpoint `cd2d920` gives 3,673 / 3,954; `6418e6b`, the parent
of `8bbb56c`, still gives 3,673 / 3,954; **`8bbb56c` first gives 4,052 / 4,354**. This isolates the increase
to the packed aggregate-result ABI, rather than the fresh-name change or the later prototype-call checks.
The following remeasures every long-double/formatting case in the earlier corpus comparison, including
its two non-increases. `gcc` denotes torture `execute`, `ieee` its `-fno-inline` subcorpus, `cts` c-testsuite.

| program | before inlining (`e8a9a95`) | packed result (`8bbb56c`) | extra steps | change |
|---|---:|---:|---:|---:|
| cts `00174` | 50,015 | 50,744 | +729 | +1.4576% |
| cts `00175` | 23,608 | 23,638 | +30 | +0.1271% |
| cts `00195` | 7,603 | 7,769 | +166 | +2.1833% |
| cts `00204` | 1,068,923 | 1,089,594 | +20,671 | +1.9338% |
| gcc `20020413-1` | 3,298 | 3,298 | +0 | +0.0000% |
| gcc `20030914-1` | 4,411 | 4,555 | +144 | +3.2646% |
| gcc `920501-8` | 13,270 | 13,276 | +6 | +0.0452% |
| gcc `930513-1` | 5,238 | 5,244 | +6 | +0.1145% |
| gcc `960215-1` | 3,863 | 4,052 | +189 | +4.8926% |
| gcc `960513-1` | 2,671 | 2,776 | +105 | +3.9311% |
| gcc `regstack-1` | 4,145 | 4,354 | +209 | +5.0422% |
| gcc `stdarg-2` | 2,754 | 2,763 | +9 | +0.3268% |
| gcc `struct-ret-1` | 29,112 | 29,610 | +498 | +1.7106% |
| ieee `20010226-1` | 2,229 | 2,239 | +10 | +0.4486% |
| ieee `920810-1` | 5,001 | 4,990 | -11 | -0.2200% |

The baseline is held at one exact commit, rather than the earlier mixed snapshot with the startup fix
applied: hence `20030914-1` is 4,411 here, rather than the previously recorded 4,427. Startup/exit counters
are unchanged in these endpoint pairs. The newly supported complex-helper case `20070614-1` has no
successful pre-inlining baseline and is not called a long-double regression.

**Instructions, measured rather than inferred from the source diff.** A probe loads volatile
`long double a=1.25L, b=0.75L`, computes one operation and returns the low 64 result bits XOR the high
16 bits (padding masked out). It is lowered from clang `-O0 -Xclang -disable-O0-optnone
-fno-math-errno`; the entry takes no arguments. For example:

```c
volatile long double a = 1.25L, b = 0.75L;
unsigned long op(void) {
    union { long double f; unsigned long w[2]; } u;
    u.f = a + b; /* also a-b, a*b, a/b, __builtin_sqrtl(a) */
    return u.w[0] ^ (u.w[1] & 65535);
}
```

All five result words equal native. Counts include the
common operand loads and bit extraction, so they are whole-probe counts, not replacement values for the
operation-cost table above.

| operation | old steps | new steps | executed instruction delta |
|---|---:|---:|---|
| add | 70 | 79 | +3 casts, +3 binary, +1 select, +1 call, +1 return = **+9** |
| sub | 117 | 127 | +3 casts, +4 binary, +1 jump, +1 call, +1 return = **+10** |
| mul | 67 | 67 | 0 |
| div | 134 | 134 | 0 |
| sqrt | 148 | 148 | 0 |

The device text shows the cause: an extF80 result crosses a surviving helper boundary as `i80`.
Packing two fields uses two `zext`, a `shl` and an `or`; extraction uses `trunc`, `lshr`, `trunc`.
LLVM also leaves `softfloat_roundPackToExtF80` out of line after this ABI change, where it was previously
inlined. The probes' added call/return pair and bit operations occur at that boundary; the add/sub
control-flow differences account for the remaining select/jump and arithmetic. This depends on operands
and inlining: it is not a fixed surcharge for every long-double operation.

The complete corpus profiles reconcile the larger counts too. For `960215-1`, +131 casts, +96 binary,
+21 calls, +21 returns, +8 jumps, +6 comparisons and +5 selects, less 34 allocas, 31 loads and 34 stores,
sum to **+189**. For `regstack-1`, +137 casts, +103 binary, +23 calls/returns each, +10 jumps, +6 comparisons,
+6 selects, less the same 34/31/34 memory operations, sum to **+209**. Its newly separate round-pack
routine executes 23 times (299 steps); `960215-1` calls it 21 times (273 steps). In `00204`, round-pack
executes 2,372 times (31,749 steps); the net **+20,671** includes savings elsewhere, chiefly `fmt_fp` and
`__mulxf3` (-3,729 each), rather than being 20,671 extra arithmetic operations. The counting helper is
`experiments/c-corpus/profile-counts.py`.

**What it buys:** small aggregate results no longer write a return buffer, so these routines can be
called under qubit control; it also avoids `a9ce585`'s code duplication and stack blow-up. The transfer
work is a cost of that chosen ABI. Repacking and less favorable LLVM inlining are optimization debts,
not additional accuracy or rounding guarantees. These are classical executed device steps on these
inputs, not native cycles, gates, or a proof that this cost is necessary.

## Global state

SoftFloat keeps the rounding mode, the tininess rule, the 80-bit rounding precision and the exception flags in
globals. The first three are never written; once the runtime is internal to the program LLVM folds their loads
to the initial values (round to nearest even, tininess after rounding as x86 detects it, 80-bit precision as
Linux sets x87). The flags are only ever or-ed into: `libgcc.c` supplies a `softfloat_raiseFlags` that does
nothing, SoftFloat's direct writes to the flags are deleted by the optimiser because nothing reads them, and
the front re-infers memory effects afterwards. A routine that returns an aggregate (SoftFloat's
`extFloat80_t`, a 128-bit integer as C returns it on x86-64) returns it as one integer of at most 128 bits,
as the front does for every aggregate result that fits (`front/src/Abi.h`), not through memory. So no routine
writes memory, and floating-point code runs in control code, under a qubit branch, and inside a coherent
region alike (`fmal` and the 128-bit conversions were refused under qubit control at `-O2` while such results
came back through memory). C can observe the flags and the
rounding mode only through `<fenv.h>`, which belongs to the C library; `#pragma STDC FENV_ACCESS` (constrained
intrinsics) is refused by name.
