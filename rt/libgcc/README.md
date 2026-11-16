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
`complex_boundaries.c` compares raw result bits (only NaN bits may differ) at the helpers' scaling
thresholds, product overflow and subnormal edges, through the helpers and `*`/`/` at both levels;
at 20,000 tuples per format (480,000 results per level) it found no other difference.
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
Results are compared bit for bit, except by **the NaN payload rule** (the maintainer's decision of the
afternoon of 2026-11-16): a result component that differs only in which NaN's payload it carries counts as
agreement. Exactly: both components are NaNs, and each carries (quiet bit set, sign free) the payload of
one of the tuple's NaN operands or of the format's default NaN, which an invalid operation inside the
helper makes. `compare.py` regenerates every tuple's operands as `cplx_exact.c` draws them, counts such
tuples separately, and fails the runner on any other difference; `compare.py --raw` keeps every raw-bit
difference a failure, for the diagnostics below, and `compare.py --self-test` checks the rule on 14 cases
worked by hand (front test `the_nan_payload_rule_allows_exactly_another_nans_payload`).
Scratch output goes under `.tmp/libgcc-exact`.

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

Independent testing found an acceptance gap in the rule: differing signalling output NaNs are
normalised to quiet NaNs before comparison and accepted. This violates the quiet-output condition
above. Ignored witness `a_signalling_output_is_not_accepted_as_another_quiet_nan_payload` in
`front/tests/tests/nan_rule_independent.rs`; route: require quiet output bits before comparing payloads.
The independent passing test in that file constructs payload and sign choices for all three formats.
This is a comparator defect; no helper output was found newly wrong by that probe.

## Raw results against the native archive (2026-11-16)

Each row is 160,000 directed tuples plus 262,144 random tuples (422,144 per helper,
2,532,864 in all), from unoptimized but optimizable input. Every component is compared as
bits, including NaN sign/payload; x87 padding bytes are excluded.

| helper | identical complex results | agree by the NaN payload rule | any other difference |
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

The 50,952 float/double tuples that differ in raw bits all agree by the NaN payload rule, and
the sweep exits **0**: measured again on 2026-11-16 (machine step e7) with an `-O0` and an `-O2`
caller, 2,898,176,872 and 2,272,217,960 device steps, the same counts per helper at both levels, none
differing otherwise. The rule needs the default NaN: without it 1,476 of 164,096 `__divdc3` tuples of a
smaller sweep fail, the native result being the default NaN of an inner `0/0` where the device carries an
input NaN (tuple 9 below); with a wrongly seeded operand generator 108 random tuples fail, so the rule is
checked against the operands actually drawn. Raw-bit agreement (`--raw`) still fails on these tuples: the
diagnosis below attributes them to native code generation's NaN selection latitude, which C and LLVM leave
open. The bit-for-bit requirement is dropped by the same decision; were it ever wanted again, the route is
to preserve the archive's per-operation operand choices through helper compilation and SoftFloat lowering,
with unchanged upstream source.

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

The other half is checked against the archive itself (`exact/commute.py`, native only, about 3 s;
front test `the_archive_nan_choices_are_operand_orders_of_the_production_ir`). It routes every
`float`/`double` `fadd`/`fmul` of the production helper IR through an SSE wrapper whose operand order
is chosen per instruction, then searches for **one fixed order per helper**, as a compiled program has.
For each of the four helpers one exists, and with it the production IR reproduces the GCC archive's
raw bits, NaNs included, on **all 422,144 tuples**: swapped instructions `0x18` of 12 (`__mulsc3`,
`__muldc3`), `0x47` of 24 (`__divsc3`; any of its lowest three may be swapped: two squares and the
denominator, whose NaN never reaches a result) and `0xe300048000` of 56 (`__divdc3`), numbered in IR order.
For `__mulsc3` the two swapped instructions are the IR's `b*c` and `bc+ad`; GCC's archive computes
`c*b` and `ad+bc` (`mulss %xmm1,%xmm11`, `addss %xmm11,%xmm5`). Together with the operand-order
diagnostic above, every one of the 50,952 differences is an operand order and nothing else.

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
(signalling NaNs), F.3 (mapping arithmetic to IEC 60559), and F.10 paragraphs 11–13
(NaN results and recommended propagation), does not impose the archive's selection.
The IEEE working group's [NaN propagation background paper, page 5](https://grouper.ieee.org/groups/msc/ANSI_IEEE-Std-754-2019/background/nan-propagation.pdf)
explains the unspecified choice among input NaNs and the compiler operand swaps that
change it. See also the C committee's
[discussion of that recommended practice](https://www.open-std.org/jtc1/sc22/wg14/www/docs/n1486.htm).
[LLVM 18's NaN rules](https://releases.llvm.org/18.1.6/docs/LangRef.html#behavior-of-floating-point-nan-values)
explicitly allow an arbitrary result sign and payload selection from a NaN operand.
Here that latitude is supported by executed native witnesses and by the full
operand-order comparison, not assumed from the mere presence of NaNs.

`__divxc3` needs no additional helper, C library function or build step for canonical x87
inputs in the default environment. Noncanonical encodings created through raw memory (unnormals,
pseudo-denormals/infinities/NaNs) are read as x87 reads them since 2026-11-16, by the floating-point
runtime at every operation's operands (`rt/softfloat/README.md`, "Noncanonical long double operands").
The remaining limitation belongs to floating-point lowering as a whole: non-default rounding and
observable flags need the front's environment fallback, real SoftFloat state and constrained-operation
lowering. That is a debt, not coverage proved by this canonical-input sweep.

Measured by the independent testing pass (2026-11-16): 800 tuples per class and helper, one
noncanonical operand at each position among canonical ones, `-O0` caller, raw bits against the archive.
Columns: identical / only NaN bits differ / other difference.

| operand class | `__mulxc3` | `__divxc3` |
|---|---|---|
| unnormal | 159 / 52 / 589 | 206 / 47 / 547 |
| unnormal zero (exponent ≠ 0, significand 0) | 240 / 66 / 494 | 334 / 43 / 423 |
| pseudo-denormal | 800 / 0 / 0 | 788 / 0 / 12 |
| pseudo-infinity | 104 / 17 / 679 | 334 / 58 / 408 |
| pseudo-NaN | 405 / 395 / 0 | 117 / 679 / 4 |

x87 refuses unnormals, pseudo-infinities and pseudo-NaNs as invalid operands (a default NaN, and an
unordered comparison); the device computed with them, giving finite or infinite values where x87 gives
NaN, and zeros of the other sign. A pseudo-denormal has x87's value, but compared below the smallest
normal on the device, so `__divxc3` could take the other Smith branch: one ulp apart. The cause was not
in the helpers: SoftFloat takes encodings at face value. Since the runtime checks every operand as x87
does, the front test `noncanonical_x87_operands_agree_with_the_hardware`, one witness per class, runs by
default and agrees in all 20 result words at `-O0` and `-O2` (16 differed before).

## Verification snapshot

All four repository checks passed after rebasing onto `3418e51` on 2026-11-16:
`cargo test` (292 passed, 19 ignored), `cargo clippy --all-targets -- -D warnings`,
`cargo fmt --all --check`, and `front/test.sh` (158 passed, 12 ignored).
These green checks do not replace the strict raw sweep's exit 1 or the normal corpus
route's three environment refusals. The large raw sweep used an `-O0` caller with the
production `-O2` helper bitcode; both caller levels are covered by the fast complex suite.
The independent testing pass then ran the same sweep with an `-O2` caller
(`run.py --random 262144 --level O2`): the same counts as the table above, no other difference,
2,272,217,960 steps (strict exit 1 for the NaN bits, as above).
The end-to-end NaN diagnostic also passes: every full-sample operand-order hardware
result matches the device, and the five small device witnesses match in **4,120 steps**.
That diagnostic success is a cause check, not an archive-exactness pass.
