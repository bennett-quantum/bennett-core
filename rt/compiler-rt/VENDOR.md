# Vendored: compiler-rt's complex multiply and divide helpers

`compiler-rt-18.1.8/` is an unmodified copy of files from LLVM's compiler-rt, release 18.1.8:
`https://github.com/llvm/llvm-project`, tag `llvmorg-18.1.8`, commit `3b5b5c1ec4a3095ab096dd780e84d7ab81f3d7ff`
(2024-06-15). Licence: Apache License 2.0 with LLVM Exceptions, `compiler-rt-18.1.8/LICENSE.TXT`.

Copied, at their upstream paths under `compiler-rt/`: `LICENSE.TXT`, and from `lib/builtins/` the six helpers
`mulsc3.c`, `divsc3.c`, `muldc3.c`, `divdc3.c`, `mulxc3.c`, `divxc3.c` and the headers they include,
`fp_lib.h`, `int_lib.h`, `int_math.h`, `int_types.h`, `int_util.h`, `int_endianness.h`. Left out: everything
else in compiler-rt (the quad-precision `multc3.c`/`divtc3.c`, which have no use while `__float128` is
refused).

These files are identical at tag `llvmorg-18.1.3`, the release of this machine's clang 18 and of its
`libclang_rt.builtins-x86_64.a`. Upstream `main` at `286ec7a` (2026-11-15) computes the same way (its changes
to these files are casts, include guards and moved helpers).

SHA-256 of the copied files:

```
1a8f1058753f1ba890de984e48f0242a3a5c29a6a8f2ed9fd813f36985387e8d  LICENSE.TXT
f6ebfa99e5a208078e998cf87e539f25a92d86e4e316fe983f4031f1745d938a  lib/builtins/divdc3.c
840b134538c700eabd1f830b400189f7e7148b90c0ffc3c70404050d46161d3e  lib/builtins/divsc3.c
2f7814e11d2866c7c2504ef6cc9c223f9f56183f1b2d510269a2c728be5f91c5  lib/builtins/divxc3.c
ce8a20f81f2e13b9ac53423c065153531f439cdbee8815df50f3ba113db51ae5  lib/builtins/fp_lib.h
ec5175561075845b7b16a92ad5ecdfd42a284de2736dd07e4ca7011b75c37709  lib/builtins/int_endianness.h
b31130f10484636895daa13b91b63d47ea82b45547f29e4dd136b48a67854d30  lib/builtins/int_lib.h
97d4f8e0be9cd48ff3153ca7ca5e88383e699c9c487fdb0c56dd2d26636c2296  lib/builtins/int_math.h
26517abcfb05e989f4ac18b38779bc118637e0a5b3e44536c3733cc47b33026e  lib/builtins/int_types.h
63d06198ab720a75c20bfd84be895c2746581b129168bae6201e7384bbb0473b  lib/builtins/int_util.h
6b5c3fba3f59901c717168ba3a51dab26674541ac16a7b870ce7af0f3e339644  lib/builtins/muldc3.c
ced55a7c473dbde086ca3649c431dc198348cf66d612eec2316862837a84b14a  lib/builtins/mulsc3.c
9de668cf8933db694755bbe4c68d0908c304c0dbbb25aee99e688cdd57d1f09c  lib/builtins/mulxc3.c
```

## What the helpers need

Measured by compiling each helper to LLVM IR with the build's flags (`front/CMakeLists.txt`) and listing what
it declares:

| helper | needs |
|---|---|
| `__mulsc3`, `__muldc3`, `__mulxc3` | nothing: arithmetic, comparisons, `fabs`, `copysign` (bit operations in the front) |
| `__divsc3`, `__divdc3` | nothing: `fp_lib.h` supplies its own `logb`, `scalbn` and `fmax` for `float` and `double` (`__compiler_rt_logbX`, `__compiler_rt_scalbnX`, `__compiler_rt_fmaxX`, inline) |
| `__divxc3` | the C library's **`logbl`** and **`scalbnl`** (`int_math.h` maps `crt_logbl` and `crt_scalbnl` to the builtins, which become library calls), and `fmaxl` (`llvm.maxnum.f80`, which the floating-point runtime supplies) |

So only the `long double` divide depends on the C library. musl's `scalbnl` is plain C, but musl's `logbl`
returns `ilogbl(x)` for finite non-zero `x`, and `ilogbl.c` is one of the twelve musl files under
`#pragma STDC FENV_ACCESS ON`, whose constrained operations the front refuses (`rt/musl/README.md`).
`__divxc3` is therefore refused by name, at `ilogbl`, with musl linked, and at `logbl` (no definition) without
it: a debt, removed by lowering constrained operations in the device's fixed environment.

Do not edit these files. Bennett's build of them is `front/CMakeLists.txt` (`complex-runtime.bc`); how they
reach a program is `front/src/Complex.h`. To update: copy the same paths from a new release and record it here.
