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
the build script and compiler flags; unchanged builds reuse the members.

Compile, resolve the archive members, and lower one C program:

```sh
python3 rt/musl/link.py program.c -o program.bn
cargo run -p bennett -- run program.bn --entry __bennett_entry --budget 100000000
```

`link.py` uses musl's public headers, preserves the program's linkage, and pulls archive members as the
probe's static-link experiment does (`resolve.py`). It resolves undefined weak references to null except
the standard init/fini bounds supplied by the front, and retries library dependencies introduced by LLVM
(such as `puts` from `printf`). It never merges the entire library or internalizes the whole program.
`--clang-flag=FLAG` passes a compiler option; `--libdir`, `--front`, `--stack-bytes`, `--region-budget`
and `--arena-bytes` select build products and limits. Intermediate files are temporary unless `--keep DIR`
is supplied. The supplied program needs a defined `int main` with zero to three ordinary C parameters.

`start.c` gives musl's unmodified `__libc_start_main` a correctly typed adapter to `main`, `argc=1`,
`argv={"prog",0}`, an empty environment and a page-size auxiliary vector. Musl initializes its own thread
and TLS data, walks the front's constructor arrays, calls main, and runs its own `exit`: atexit handlers,
destructors and stdio flushing. Bennett supplies only the thread-pointer bridge and system-call boundary;
there is no probe hash oracle or hand-made exit sequence.

## The boundary

The device has empty stdin and one console. Reads of fd 0 (`read`, `readv`) report end of file.
Writes/writev to fd 1 append bytes with `__bennett_effect`; a vector is checked whole before any byte is
written. **A write of one or more bytes to fd 2 faults as `unreachable`**: stderr cannot disappear silently,
and cannot be mixed into the stdout-only comparison. Closing a standard fd is remembered; closed and
unsupported descriptors fail with `EBADF`. The console is not a terminal (`ioctl` gives `ENOTTY`), and
seeking it gives `ESPIPE`.

`exit`/`exit_group` finish through `__bennett_exit`, including from nested calls. The arena defaults to 1 MiB;
`--arena-bytes` is a link parameter in whole 4096-byte pages. `brk` grows from the bottom and anonymous
private mappings from the top; without `MAP_FIXED` an address is only a hint. Exhaustion returns `ENOMEM`;
`mremap` fails with `ENOMEM` (musl's `realloc` then copies). The arena is monotone: `munmap` accepts valid
ranges but does not reclaim storage; use after release is outside the C allocation lifetime. A `MAP_FIXED`
anonymous mapping is accepted over arena pages already handed out and gives fresh zero pages there (musl's
`malloc` maps its guard page this way); elsewhere it traps. Pages the break gives back read as zero when it
grows over them again, and `MADV_DONTNEED` zeros whole pages; the hints `MADV_NORMAL`, `RANDOM`,
`SEQUENTIAL` and `WILLNEED` are accepted, other advice traps. Protections `PROT_NONE`, `PROT_READ` and
read/write are accepted by `mmap` and `mprotect` but not enforced: a program that passes natively never
touches memory it may not, and one that does would fault natively where the device goes on. Executable,
file-backed and shared mappings trap. This is a bounded allocation backend, not a host virtual-memory
implementation.

Signals are the ones a program sends itself (`raise`, `kill` or `tkill` of process and thread 1); there are
no asynchronous signals. They are delivered as the kernel delivers them, before the system call that raises
or unblocks them returns: dispositions, the signal mask, the pending set (`sigpending`) and installed
handlers, with `SA_SIGINFO` (a `siginfo_t` with the signal number and `SI_TKILL`; the context is zero),
`SA_RESETHAND`, `SA_NODEFER` and the handler's mask. A default action terminates with status 128 plus the
signal number without flushing stdio (`abort` gives 134), or ignores `SIGCHLD`, `SIGCONT`, `SIGURG` and
`SIGWINCH`; stop signals trap. A second real-time signal raised while one is pending traps (it would queue).
Hardware signals have no counterpart: a division by zero is the device's named fault, not `SIGFPE`.
Process and thread ids are 1. Opening files and every unimplemented system call trap as `unreachable`.
Threads, clocks, file I/O and dynamic loading are not implemented. No unsupported operation is turned into a
successful no-op with an observable result.

## Floating point

Measured before selecting the flags: the existing front refuses musl's `sin.c` built with
`-frounding-math` at `llvm.experimental.constrained.fpext.f64.f32`. With `-fno-rounding-math` it passes
floating-point lowering and waits on the ordinary library dependency `__rem_pio2`. The product build
therefore uses `-fno-rounding-math`, leaving the front's refusal of constrained operations intact. It uses
SoftFloat's existing round-to-nearest-even mode; dynamic floating-point environments are not supported.
This is a build choice within that existing layer, not a new floating-point promise.

`<fenv.h>` is Bennett's (`runtime.c`), not musl's: musl's generic dummy `fenv.c`, built where x86-64 has
assembly, reported success for `fesetround(FE_UPWARD)` without changing the rounding and never saw a flag,
while the x86-64 headers name every mode and flag (a measured silent wrong result). The default environment
is supported: `fegetround` gives `FE_TONEAREST`, `fesetround(FE_TONEAREST)`, `feclearexcept`, `fegetenv`,
`feholdexcept` and `fesetenv` of the default or a saved environment succeed. Another rounding mode, raising
or testing a flag, and setting another environment trap as `unreachable`. Twelve musl math files use
`#pragma STDC FENV_ACCESS ON` (`ilogb`, `ilogbf`, `ilogbl`, `fma`, `fmaf`, `fmal`, `lrint`, `lrintl`,
`llrintl`, `nearbyint`, `nearbyintf`, `nearbyintl`); the front refuses their constrained operations by name. Neither of the probe's
integer-only printf/scanf patches is applied: every upstream file, including floating-point stdio, is intact.

## On the corpora

Measured on 2026-11-15 at `b961292` and again at `8bbb56c`, with the same outcome for every program
(`experiments/c-corpus/README.md`, "The three corpora at `8bbb56c`" and "at `b961292`"):
on GCC's C torture tests 1,560 of 1,575 runnable programs agree with native on status and stdout, on
c-testsuite 217 of 220, on GCC's `ieee/` 64 of 67; none is wrong and none crashes. What still does not run,
and what removes it:

| refused or faulted | programs | what removes it |
|---|---:|---|
| files: `open`, `fopen`, `freopen` trap by name | 6 | a file model at this boundary |
| `setjmp`, `_setjmp`, `longjmp`: no definition (musl's are x86-64 assembly, not built) | 2 | removed: the device's own `setjmp` and `longjmp` (`docs/DESIGN.md`, "Non-local jumps"); both agree, run alone |
| `__mulsc3`, `__divsc3`: no definition | 2 GCC | removed: complex helpers (now libgcc, `rt/libgcc`); both agreed at `b961292` |
| musl's `ilogb`, `ilogbl` (and ten other math files) under `#pragma STDC FENV_ACCESS ON`: constrained operations refused by the front | 3 `ieee/` (`cdivchkf` was counted at `__divsc3` before the helpers) | lowering constrained operations in the device's fixed environment (to nearest, no readable flags), which `fenv` here already enforces. The six helpers now come from libgcc (`rt/libgcc`), including self-contained `__divxc3`; the corpus programs' own `ilogb`/`ilogbl` calls still need this fallback |
| a program's own `int strlen(char *)` against the library's (c-testsuite `00025`) | 1 | a call through a mismatched prototype, a front refusal |
| not library calls: `llvm.returnaddress`, inline assembly, an over-aligned variadic argument, an unprototyped call with an extra argument (`bad_indirect`), a run past the step budget (`float` to `i128` was one; it agrees since the 128-bit conversions) | 7 (9 before `llvm.eh.sjlj.longjmp` became the device's `longjmp`) | the front's own debts (`HANDOFF.md`) |

## Tests and text size

```sh
python3 rt/musl/test.py                   # needs the library build, front, and debug CLI already built
```

These offline integration tests are separate from `cargo test` and `front/test.sh`. They compare native
stdout/status for floating-point printf/strtod/sqrt/sin/pow, for constructors, atexit, destructors and
nested exit, for `malloc`/`realloc`/`calloc`/`free`, for signals raised, blocked, ignored and handled, for
`abort` after a handler returns, for reads of the empty stdin through `scanf`, `getchar` and `fgets`, and for
a break that shrinks and grows again, for the default floating-point environment, and for complex products,
quotients and `cexp` (`complex.c`, over libgcc's helpers), and for `setjmp`/`longjmp` across frames, `sigsetjmp` with
and without the signal mask (musl's `__sigsetjmp_tail`) and jumps out of signal handlers (`setjmp.c`); they check arena
exhaustion and named faults for stderr, an unimplemented syscall, another rounding mode and a flag test.
`tests/regress/` holds the self-checking witnesses of the independent testing pass of this boundary (each
returns 0 natively and failed or faulted here before its fix). The 24 tests took 13 to 30 seconds with the
debug CLI after building the library.
The debug CLI now reads their large data strings without overflowing its host stack.

The front writes zero runs of 16 bytes or more as one `zero N` data item, so the 1 MiB arena costs a
line, not a megabyte of explicit zeros. Measured at `6da7ef6` with `link.py` and default limits: a `printf`
hello world is 215,785 bytes of device text, and the library example (`tests/library.c`, with
floating-point `printf`, `strtod`, `sqrt`, `sin` and `pow`) 652,468 bytes, of which 26 `zero` items reserve
1,054,135 zero bytes; the same example was about 4.8 MB before `zero N`.
