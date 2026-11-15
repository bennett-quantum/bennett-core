# musl 1.2.5 for Bennett

The upstream release is vendored unmodified; its licence and archive identity are in `VENDOR.md`.
The three overlay headers replace x86-64 system-call assembly, atomics and the thread pointer for the
single-threaded device. No network is used by the build.

Build the library's bitcode archive members with LLVM/clang 18, GNU make and Python 3:

```sh
python3 rt/musl/build.py                  # target/musl; an output directory may be supplied
```

The build configures musl out of tree, uses its own `LIBC_OBJS` selection, and substitutes its generic C
files for x86-64 assembly wherever present. It produces 1,345 bitcode members, a member list, `libc.a`,
installed public headers and JSON compiler flags. Its fingerprint includes all upstream files, the overlays,
the build script and compiler flags; unchanged builds reuse the members. Product linking and startup are
the next step; this archive must be resolved by member, not linked as a whole module.

Measured before selecting the flags: the existing front refuses musl's `sin.c` built with
`-frounding-math` at `llvm.experimental.constrained.fpext.f64.f32`. With `-fno-rounding-math` it passes
floating-point lowering and waits on the ordinary library dependency `__rem_pio2`. The product build
therefore uses `-fno-rounding-math`, leaving the front's refusal of constrained operations intact. It uses
SoftFloat's existing round-to-nearest-even mode; dynamic floating-point environments are not supported.
This is a build choice within that existing layer, not a new floating-point promise. Neither of the probe's
integer-only printf/scanf patches is applied: every upstream file, including floating-point stdio, is intact.
