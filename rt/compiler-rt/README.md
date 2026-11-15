# rt/compiler-rt: the complex multiply and divide helpers

clang computes a product or quotient of `_Complex float`, `double` or `long double` values by calling
`__mulsc3`, `__divsc3`, `__muldc3`, `__divdc3`, `__mulxc3` or `__divxc3` (a quotient always; a product is computed
inline first and the helper called only when both parts of that result are NaN). The helpers hold C's Annex G
recovery of infinities and NaNs (a nonzero value over zero is infinite, an infinity times a nonzero value is
infinite, ...). Natively the compiler runtime defines them. Here they are compiler-rt's own sources, vendored
unmodified (`VENDOR.md`), as decided by the maintainer on 2026-11-15 (`HANDOFF.md`).

CMake compiles the six helpers to bitcode with their floating-point operations intact and links them into
`target/front/complex-runtime.bc`; `bennett-front --complex-runtime file.bc` takes any bitcode defining the same
names. Before lowering floating point, the front links the helpers a program calls and does not define
itself, as a static linker pulls archive members, and makes them internal; they are then lowered with the
program over the floating-point runtime (`rt/softfloat`), and the optimiser inlines them where it sees fit
(`front/src/Complex.h`). A helper defined with another type is refused by name.

What each helper needs, and why `__divxc3` stays refused, is in `VENDOR.md`: the `float` and `double` helpers
and `__mulxc3` are self-contained; `__divxc3` calls the C library's `logbl`, which musl computes with `ilogbl`,
whose `#pragma STDC FENV_ACCESS ON` the front refuses. That refusal is a debt; it goes when constrained
operations are lowered in the device's fixed environment (`rt/musl/README.md`).

## Bit for bit against the same helpers on this machine's hardware

Measured on 2026-11-15 on x86-64 (Zen 2, glibc 2.39, clang 18.1.3). Each helper on the cross product of 16
directed values per format (±0, 1, -3, 0.75, the smallest subnormal, the smallest normal, the largest finite
value negated, large and small powers of two, ±infinity, a quiet NaN and a negative NaN with a payload: 65,536
tuples) and 16,384 random tuples (raw bits, exponents near 1, exponents in range, directed values mixed in;
canonical encodings for `long double`), 81,920 tuples per helper, 491,520 in all: on the device (linked with
musl by `rt/musl/link.py`, one run of 539,408,695 steps, 11 s with the release executor) against the vendored
sources compiled natively with `clang -O2` (the same at `-O0`, and the same as this machine's
`libclang_rt.builtins-x86_64.a`, release 18.1.3).

| helper | identical | NaN result with other NaN bits | other differences |
|---|---:|---:|---:|
| `__mulsc3` | 80,652 | 1,268 | 0 |
| `__divsc3` | 75,856 | 6,064 | 0 |
| `__muldc3` | 80,058 | 1,862 | 0 |
| `__divdc3` | 75,890 | 6,030 | 0 |
| `__mulxc3` | 81,920 | 0 | 0 |
| `__divxc3` | 77,868 | 4,052 | 0 |

`__divxc3` was measured with a test-only `logbl` computed from the bits (musl's `logbl` with `ilogbl`'s result
taken from the exponent, without its `FENV_ACCESS` pragma), in the native build and on the device alike; it
measures the helper and its lowering, not a route.

Every result that is not a NaN is bit for bit the native one; a NaN result is a NaN, as the floating-point
runtime promises (`rt/softfloat/README.md`). The NaN bits differ in two ways, both left open by LLVM, which
fixes the sign of a NaN only for `fneg`, `fabs` and `copysign`: which of two NaN operands an operation returns
(the operand order is the optimiser's choice), and the sign. For example `__divsc3(0, 0, 0, NaN)`'s imaginary
part is `b·c - a·d`, which LLVM writes as `b·c + (-d)·a`; x86's code generator folds that negation into a
subtraction (`subss`, checked with `llc`), which keeps the NaN's sign, while the device negates the bit first.

## Not libgcc

A native program built by default on Linux gets its helpers from libgcc, whose division is a different
algorithm. On the same 81,920 tuples per helper, libgcc's results against compiler-rt's, excluding NaN bits:

| helper | results that differ (last bit, sign of a zero) |
|---|---:|
| `__mulsc3`, `__muldc3`, `__mulxc3` | 0 |
| `__divsc3` | 10,023 |
| `__divdc3` | 13,555 |
| `__divxc3` | 13,800 |

GCC's `ieee/` tests `cdivchkf`, `cdivchkd` and `cdivchkld` check complex division to within a few bits at
known hard operands. Built natively with `clang --rtlib=compiler-rt` (compiler-rt's helpers), all three abort,
at `-O0` and `-O2`; with libgcc they pass. On the device with a test-only `ilogb` in place of musl's they
abort too (status 134), as natively with compiler-rt. The corpus compares with a libgcc-linked native build,
so once the `FENV_ACCESS` refusal that stops them today is lifted, these three will be counted as wrong
results unless the helpers or the reference change. Upstream compiler-rt's `main` (2026-11-15) divides the
same way.

## Tests

`front/tests/tests/complex.rs` (in `front/test.sh`, about 8 s): every helper but `__divxc3`, called directly
and through `*` and `/`, on the cross product of eight directed values per format, at `-O0` and `-O2`,
against the natively compiled helpers, 64 results hashed per group with NaNs counted as one value (640
groups). A device runtime whose `__divsc3` or `__mulsc3` drops the Annex G recovery changes 118 or 110 of the
groups; libgcc's helpers as the reference would change 180. It also checks the refusal of `__divxc3` without
a C library, that a program's own definition of a helper is kept, that a program calling none links none, and
that `--complex-runtime` replaces the helpers and refuses a missing one, a wrong type and an unreadable file
by name. `rt/musl/tests/complex.c` runs products, quotients, `cexp` and an infinite quotient with musl linked.

To repeat the measurement (`exact/`; `test_logbl.h` is the test-only `logbl`, `compare.py` classifies):

```sh
cd rt/compiler-rt/exact && B=../compiler-rt-18.1.8/lib/builtins && mkdir -p out
for f in mulsc3 divsc3 muldc3 divdc3 mulxc3 divxc3; do clang-18 -O2 -c $B/$f.c -o out/$f.o; done
clang-18 -O2 -DNATIVE -include test_logbl.h cplx_exact.c out/*.o -lm -o out/native && out/native > out/native.txt
cat test_logbl.h cplx_exact.c > out/device.c && python3 ../../musl/link.py out/device.c -o out/device.bn
../../../target/release/bennett run --entry __bennett_entry --budget 100000000000 out/device.bn |
  sed -n 's/^console: "\(.*\)"$/\1/p' > out/device.txt
python3 -I compare.py out/device.txt out/native.txt
```
