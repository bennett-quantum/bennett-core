# Vendored: Berkeley SoftFloat 3e

`berkeley-softfloat-3e/` is an unmodified copy of Berkeley SoftFloat, Release 3e, by John R. Hauser:
`https://github.com/ucb-bar/berkeley-softfloat-3`, commit `f74b1e4` ("Release 3e", 2018-01-26). Licence:
BSD 3-clause, `berkeley-softfloat-3e/COPYING.txt`.

Copied: `COPYING.txt`, `README.md`, `source/*.c`, `source/include/`, the `source/8086-SSE/` specialisation
(x86 SSE NaN behaviour) and `build/Linux-x86_64-GCC/` (its `platform.h` and `Makefile`, which list the
build). Left out: the other specialisations and build directories, and `doc/`.

Do not edit these files. Bennett's own code (the compiler runtime-call interface over SoftFloat, and the
build) lives next to this directory. To update: copy the same paths from a new commit and record it here.
