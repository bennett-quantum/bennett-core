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
