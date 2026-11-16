# Complex multiplication and division over libgcc

All six helpers (`__mulsc3`, `__muldc3`, `__mulxc3`, `__divsc3`, `__divdc3`, `__divxc3`)
come from **unmodified GCC 13.3.0 libgcc**. Provenance, signatures, checksums, licences,
target configuration and the host reference audit are in [VENDOR.md](VENDOR.md).

CMake builds libgcc2.c once per helper into `target/front/complex-runtime.bc`. The front
links only helpers a program calls without defining, then lowers their floating-point
operations with the user program over SoftFloat. `--complex-runtime file.bc` replaces them.
All six are self-contained; long-double division works without a C library.

`front/tests/tests/complex.rs` compares all six helpers and ordinary C products/quotients
against the default native libgcc at `-O0` and `-O2`, including Annex G special values.
The grouped regression checks normalize NaNs according to the floating-point contract.
For a raw-bit comparison, build the front and release executor, then run:

```sh
python3 rt/libgcc/exact/run.py --random 262144 --level O0
python3 rt/libgcc/exact/run.py --random 262144 --level O2
```

The exact sweep emits raw real/imaginary result bits for the Cartesian product of 20
values (160,000 tuples, including signed zeros, infinities, quiet/signalling NaNs,
normal/subnormal limits and large/small exponent ratios) and 262,144 mixed random tuples
per helper. It compares with **the host's libgcc archive**, retaining the native link map.
`long double` inputs use canonical x87 encodings, and output omits its six padding bytes.
NaN sign/payload differences are counted separately, never hidden in an equality claim.
Scratch output goes under `.tmp/libgcc-exact`; any raw-bit mismatch fails the runner (including NaN bits).

The GCC `cdivchk` programs separately call `ilogb`/`ilogbl`. Those musl routines contain
`FENV_ACCESS` and currently require the front's constrained-operation fallback. This is
separate from `__divxc3`, which no longer calls a library function.

To repeat the corpus check, pass the GCC **14.2.0** corpus's
`gcc/testsuite/gcc.c-torture/execute/ieee` directory (build musl first):

```sh
python3 rt/libgcc/exact/corpus.py path/to/execute/ieee
```

The runner uses the corpus's `-fno-inline`, checks native `-O0` and `-O2` stdout/status,
and attempts the normal musl link first. Its separately labelled diagnostic copies musl's
`ilogb.c`, `ilogbf.c` and `ilogbl.c` into scratch, removing only their `FENV_ACCESS` pragmas;
it supplies their internal declaration visibility macro but changes no arithmetic. The
original corpus programs and all six libgcc helpers stay unchanged. This diagnostic tests
the division algorithms; it does not implement observable exceptions or turn a normal-route
refusal into a corpus pass. The runner exits **1** if any normal route refuses, even when every diagnostic agrees.
Scratch sources, native link maps and outputs are retained under
`.tmp/libgcc-corpus`.

Measured on 2026-11-16 with GCC 14.2.0's three original test files, clang 18.1.3 and
libgcc 13.3.0: both native levels exit 0 with empty stdout. All three normal device links
still refuse at musl's constrained operation. With the explicitly diagnostic pragma removal,
all three return 0 with empty stdout: `cdivchkf` **7,045**, `cdivchkd` **7,262**, `cdivchkld`
**9,238** steps. The old compiler-rt division failed even this diagnostic; libgcc passes it.
This removes the helper-accuracy failure, while the ordinary corpus route remains a debt.

## Raw results against the native archive (2026-11-16)

Each row is 160,000 directed tuples plus 262,144 random tuples (422,144 per helper,
2,532,864 in all), from unoptimized but optimizable input. Every component is compared as
bits, including NaN sign/payload; x87 padding bytes are excluded.

| helper | identical complex results | differ only in NaN bits | any other difference |
|---|---:|---:|---:|
| `__mulsc3` | 408,400 | 13,744 | 0 |
| `__divsc3` | 417,674 | 4,470 | 0 |
| `__muldc3` | 408,547 | 13,597 | 0 |
| `__divdc3` | 403,003 | 19,141 | 0 |
| `__mulxc3` | 422,144 | 0 | 0 |
| `__divxc3` | 422,144 | 0 | 0 |

The run returns 0 after **2,898,176,872** device steps; the runner's timed execution and
output handling took **228.467 s** on the shared host. A separate directed-only run
(960,000 tuples) finds the same classes, no other differences, in 1,021,327,975 steps.
Both long-double helpers match every result bit, including NaNs, with **no test-only
library function** and no musl link. All six match every non-NaN result and every zero sign.

**The requested complete raw-bit agreement is not achieved:** 50,952 tuples differ in
float/double NaN fields, and the strict sweep exits **1**. The fast suite's NaN-normalized
agreement is the existing floating-point contract, not a replacement for that requirement.
The executed diagnosis below attributes these differences to native code generation's
NaN selection latitude. They are not unexplained arithmetic lowering failures. The strict
archive-bit requirement is still recorded here rather than relabelled a full raw-bit pass.
If archive-identical NaNs remain required beyond that latitude, the route is to preserve
that archive's per-operation operand choices through helper compilation and SoftFloat
lowering, with unchanged upstream source. Merely changing the global NaN priority would
break other operations, and compiling helpers at `-O0` does not solve the problem. Using
privately recompiled helpers as the oracle would hide part of it and is not a fix.

## Executed NaN diagnosis

Repeat after the exact sweep (the diagnostic reuses its saved device output):

```sh
python3 rt/libgcc/exact/diagnose.py --random 262144 --level O0
```

The reference stays the installed GCC archive. Separate **hardware** builds of the very
same unmodified libgcc source with clang 18 show that native execution itself varies:

| hardware helper build vs GCC archive | mulsc3 | divsc3 | muldc3 | divdc3 | mulxc3 | divxc3 |
|---|---:|---:|---:|---:|---:|---:|
| Clang `-O0`, NaN-only differing tuples | 2,205 | 4,470 | 2,178 | 17,403 | 0 | 0 |
| Clang `-O2`, NaN-only differing tuples | 13,744 | 16,452 | 13,597 | 0 | 0 | 0 |

Device vs Clang `-O2` hardware has 0/16,158/0/19,141/0/0 NaN-only differing tuples
in that order. That initially looks like a separate device division defect. The cause
is **commutation of SSE operands in native code generation**. SoftFloat's 8086-SSE
specialization propagates the first NaN operand (quieting it); native SSE prioritizes its
destination operand. LLVM's x86 backend may commute `fadd`/`fmul` to reuse a register,
changing that priority without changing a non-NaN result.

To test that cause over the **entire sample**, the diagnostic disassembles the actual
production helper bitcode and changes only its `float`/`double` `fadd` and `fmul` instructions
in scratch IR into native calls. Four tiny SSE wrappers force IR operand 0 to be the
instruction destination; subtraction, division, conversions, branches and every x87
operation remain native. This **hardware execution agrees with all 2,532,864 device
results bit for bit, including every NaN**. The diagnostic asserts exact equality and
retains its scratch IR and objects. This isolates native operand choice as the cause of
all measured device-vs-same-IR differences; it does not substitute a new correctness oracle.
No arithmetic lowering defect is demonstrated by this sample.

Small executed witnesses are in `exact/nan_witness.c`. For example, for
`__divdc3(0, 0, 0, +qNaN)` (directed tuple 9), GCC's archive and Clang `-O2` hardware
return `fff8000000000000` in both components; Clang `-O0` hardware, the device and the
operand-order hardware diagnostic return `7ff8000000000000` in both. Inside the
Smith branch, `a/c` is `0/0`, producing a negative default NaN. The IR multiplies `d`
(the positive NaN) by that result; the backend commutes this multiply. For tuple 19,604,
`__divdc3(1, +qNaN, 0, smallest-subnormal)`, the numerator adds `b` to the NaN from
`0 * infinity`; a commuted add similarly explains the remaining Clang `-O0` difference.
For `__divsc3(0, 0, +qNaN, -payload-qNaN)` (tuple 190), Clang `-O2` hardware returns
real bits `ffc12345`, whereas its `-O0` build, the archive and the device return
`7fc00000`: the backend commutes the real numerator's add. All these witnesses use quiet
NaNs, so the explanation does not rely on signalling NaNs being undefined.

C does not promise a particular input NaN's payload for an arithmetic result. C11's
[Annex F](https://www.open-std.org/jtc1/sc22/wg14/www/docs/n1570.pdf), notably F.2.1
(signalling NaNs) and F.10 paragraphs 11–13 (NaN results and recommended propagation),
does not impose the archive's selection. See also the committee's
[discussion of that recommended practice](https://www.open-std.org/jtc1/sc22/wg14/www/docs/n1486.htm).
[LLVM 18's NaN rules](https://releases.llvm.org/18.1.6/docs/LangRef.html#behavior-of-floating-point-nan-values)
explicitly allow an arbitrary result sign and payload selection from a NaN operand.
Here that latitude is supported by executed native witnesses and by the full
operand-order comparison, not assumed from the mere presence of NaNs.

`__divxc3` needs no additional helper, C library function or build step for canonical x87
inputs in the default environment. Remaining long-double limitations belong to floating-point
lowering as a whole: noncanonical encodings created through raw memory (unnormals,
pseudo-denormals/infinities/NaNs) still differ from x87, and non-default rounding/observable
flags need the front's environment fallback. The former requires x87-compatible validation
and normalization of operand encodings at the operation boundary; the latter requires real
SoftFloat state and constrained-operation lowering. They are debts, not coverage proved by
this canonical-input sweep.
