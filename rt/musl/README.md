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
private mappings from the top; without `MAP_FIXED` an address is only a hint. **Exhaustion is the named fault
`out_of_memory`**, never `ENOMEM` or a null result: a growth of the break or a fresh mapping the arena cannot
hold (one larger than the arena too) calls `__bennett_out_of_memory`, which ends the run with the console as
written so far (stdio unflushed, as for every fault). Musl's arithmetic rejections before any system call
(`malloc(SIZE_MAX)`, an overflowing `calloc`, `mmap` of `PTRDIFF_MAX` bytes or more, the public `brk` and
`sbrk(n != 0)`, which musl never implements) stay null or `ENOMEM`, as natively (`docs/DESIGN.md`, "Files,
clock and entropy at the musl boundary"). `mremap` fails with `ENOMEM` (musl's `realloc` then copies); a `MAP_FIXED` mapping longer than
the arena fails with `ENOMEM`. The arena is monotone: `munmap` accepts valid
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
Process and thread ids are 1. Every unimplemented system call traps as `unreachable`.
Threads and dynamic loading are not implemented. No unsupported operation is turned into a
successful no-op with an observable result.

## Files and deterministic inputs

The carried archive models run as C on the device, with no host IO. File bytes, sparse 4096-byte
pages, names, inodes, descriptor flags, shared open-description offsets, cwd and umask are ordinary
journalled memory (`CARRIED.md` gives provenance). Regular files support open/openat/creat,
read/write/readv/writev, pread/pwrite, seek, dup/dup2/dup3, stat, descriptor/status fcntl flags,
truncate/ftruncate, readlink/readlinkat errors over the current non-symlink namespace, mkdir/rmdir, chdir/fchdir/getcwd, rename/unlink, chmod/umask and sync.
The initial namespace has `/`, `/tmp` (mode 01777), `/dev`, `/dev/random`, `/dev/urandom`, `/dev/null`, `/dev/zero` and
`/dev/console`. Duplication preserves the explicit stderr fault; fd 2 may be redirected to a file.

Errors follow Linux's order of checks where several apply (arguments before the path walk, earlier
components before later ones, a last component `.`, `..` or the root refused by kind before lookup), with
`PATH_MAX` and `NAME_MAX` enforced. The working directory and directory descriptors are inodes: they follow
renames, and once removed they name nothing (relative names then give `ENOENT`; Linux would still find `.`
and `..` there, a recorded debt). `O_ACCMODE` opens for neither reading nor writing, directories accept
`fsync`, and `SEEK_DATA`/`SEEK_HOLE` report the model's unwritten 4096-byte pages as holes, as ext4 and tmpfs
with 4 KiB blocks do. A vector with a bad element after a good one transfers the good part, except on the
console, which is checked whole. The CPU-time clocks of the caller (`clock_getcpuclockid`,
`pthread_getcpuclockid`) share the clock's counter.

Supply the run's inputs when constructing its image:

```sh
python3 rt/musl/link.py program.c -o program.bn --clock-start 1234999999998 \
  --entropy-image entropy.bin --file-image files.json
bennett run program.bn --entry __bennett_entry --reverse --stats
```

`files.json` maps canonical absolute paths to hex byte strings, for example
`{"/fixtures/input":"610062ff63"}`. Parent directories are created automatically. Installation occurs
before the first device syscall, including those called by constructors, and is itself journalled.
The defaults are no user files, clock start zero and an empty entropy image. These are inputs of
the run image; to use another image currently relink. Host paths are read only by the image builder.

A clock read returns the unsigned 64-bit start plus the number of earlier clock reads, in nanoseconds,
wrapping modulo 2^64. Accepted clock ids share that counter. `clock_gettime`, `gettimeofday` and `time`
consume one read when they request a value; `clock_getres` returns 1 ns without consuming a read.
This is independent of the number of executed instructions. `getrandom`/`getentropy` and both random
devices consume the same exact byte image. It is a seeded input, not a host entropy source or a PRNG.
Exhaustion traps as `unreachable` before writing any bytes, including on a multi-element readv.
Empty reads consume none. Undo restores both counters, destinations and all filesystem state.

Metadata uses root ownership, a model inode/device identity, and zero timestamps. Unsupported
operations and flags trap by name instead of claiming success. The remaining debts and their routes
are in `HANDOFF.md`, including locks, links, directory enumeration, per-invocation image loading,
and permissions/metadata. The existing finite arena bounds the filesystem as well as the application
heap; a file operation that exhausts it ends the run with `out_of_memory`, so no half-made change is seen.

```sh
python3 rt/musl/test-devices.py   # native file/image comparisons, arithmetic clock oracle, tape, reversal
```

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
| files: `open`, `fopen`, `freopen` previously trapped by name | 6 | carried file model at this boundary; targeted rerun results below |
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
and without the signal mask (musl's `__sigsetjmp_tail`) and jumps out of signal handlers (`setjmp.c`); they check that heap
exhaustion by `mmap`, `malloc`, `realloc` and the break is the fault `out_of_memory` with the output flushed
before it kept (`arena`, `heap-exhaust`, `realloc-exhaust`, `mmap-exhaust`, `brk-exhaust`), that musl's
arithmetic rejections stay null against native (`alloc-arithmetic`), and named faults for stderr, an unimplemented syscall, another rounding mode and a flag test.
`tests/regress/` holds the self-checking witnesses of the independent testing pass of this boundary (each
returns 0 natively and failed or faulted here before its fix). The 35 programs (42 runs with the
`-O2` ones) took 173 seconds with the debug CLI on the shared machine, after building the library.
The debug CLI now reads their large data strings without overflowing its host stack.

The front writes zero runs of 16 bytes or more as one `zero N` data item, so the 1 MiB arena costs a
line, not a megabyte of explicit zeros. Measured at `6da7ef6` with `link.py` and default limits: a `printf`
hello world is 215,785 bytes of device text, and the library example (`tests/library.c`, with
floating-point `printf`, `strtod`, `sqrt`, `sin` and `pow`) 652,468 bytes, of which 26 `zero` items reserve
1,054,135 zero bytes; the same example was about 4.8 MB before `zero N`.

### Files/clock/entropy checks (2026-11-16)

`test-devices.py`: **24 passes** (16 fixture runs at `-O0` and non-vectorized `-O2`, and the 8 witnesses of
`tests/devices-regress/`), 147.31 s on the shared machine.
Files and constructor-loaded byte images agree with native stdout/status. Clock arithmetic and seed
bytes agree with a separate native C arithmetic oracle, including start `2^64-2` wrapping through zero.
`getentropy`, random-device reads and vectored tape consumption additionally check each expected byte.
All forward runs un-run to the identical entry snapshot (including buffers, both consumption counters,
filesystem/allocator state and the lazy image guard); amplitude error zero. The witnesses also un-run exactly
from cuts at a quarter, half and three quarters of their run.

`tests/devices-regress/` holds the self-checking witnesses of the independent testing pass of files, clocks
and entropy: each returns 0 natively (ext4) and failed on the device before its fix. They cover removing or
renaming `.`, `..` and the root (`rmdir(".")` removed the directory; `rename(d, "/")` replaced the root),
trailing slashes (`rename(file, "new/")` moved the file), a working directory removed and remade under its
name (a relative create landed in the new directory), `fchdir` to a removed directory, `O_TRUNC` on a
directory, `O_ACCMODE`, `fsync` of a directory, the error order of `ftruncate`, `truncate`, `access`,
`pread` and `pwrite`, `SEEK_DATA`/`SEEK_HOLE`, `NAME_MAX`/`PATH_MAX`, partial vectored IO, and the
caller's CPU-time clock ids. Clock and entropy values matched the decided arithmetic in every probe. Empty-image exhaustion is
an expected named fault, and is also fully reversed. Representative `-O0` costs:

| fixture | steps | journal bytes |
|---|---:|---:|
| files: sparse/truncate/append/shared offsets/dup/vector IO | 79,057 | 1,632,977 |
| file errors, modes, links, umask, cwd/rename | 64,475 | 1,328,123 |
| temporary names/readlink/stdio/anonymous tmpfile | 64,625 | 1,299,601 |
| supplied file bytes read in constructor and main | 41,218 | 832,978 |
| clock start 1,234,999,999,998 and 16 entropy bytes | 30,836 | 671,892 |
| clock start `2^64-2`, wrap, same entropy bytes | 31,245 | 680,861 |
| getentropy plus random-device/readv byte checks | 42,189 | 836,029 |
| empty entropy image, expected fault | 2,101 | 41,781 |

The integration suite remains separate from fast `cargo test` and `front/test.sh`; it requires the
explicit musl build. Unsupported operations and finite tape exhaustion have not been counted as
successful native random or wall-clock calls.

Targeted corpus confirmation: **all six prior file-system faults now agree with native**
(`experiments/c-corpus/README.md`, "Files and clocks"). `printf-2` / `user-printf` / `fprintf-2` take
58,353 / 58,416 / 57,717 steps at `9194605`; `/dev/zero` loops `loop-2f` / `loop-2g` and c-testsuite
`00187` agreed earlier on the file implementation at 35,235 / 35,235 / 102,998 steps. The temporary-name
cases found musl's `readlink` probe, now routed correctly for the current non-symlink namespace.
No whole corpus was rerun and no global corpus total is inferred from these six outcomes.

Final combined-revision verification after rebasing on `40c84d4`: `cargo test` **306 passed**,
`front/test.sh` **170 passed**, clippy all targets with warnings denied and formatting clean.
The 16 focused cases passed again in **135.20 s** on the shared machine, with every step/journal count
above unchanged and every entry snapshot restored exactly. This also checks the file/clock models
against the upstream atomic-rewind fixes and libgcc runtime update.

### Copied jump buffers: glibc/musl comparison

The maintainer's reference for a copied `sigjmp_buf` is glibc, even with musl linked
on the device. `front/tests/c/sigjmp_copy_matrix.c` retains 576 cases per optimization
level: eight copy paths (memcpy, struct assignment, two union paths, record-only,
record-plus-flag, mask-only and a full copy in pieces), savemask 0/1/-7, a destination
saved with 0/1, no re-save or re-saving the original with 0/1, jumps through either
buffer, and two masks between save and jump. Every saving frame stays live. The
partial-copy offsets are explicitly x86-64 glibc/musl ABI probes, not portable C
promises about the opaque buffer. Output includes the landing site, jump-value
check and mask; the harness compares complete stdout and exit status.

Build the bitcode library and a separate, unmodified native musl from the vendored
source (no network), from the repository root:

```sh
python3 rt/musl/build.py --jobs 3
mkdir -p target/musl-native/build
(cd target/musl-native/build && \
  ../../../rt/musl/musl-1.2.5/configure --disable-shared --prefix="$PWD/../install" CC=clang-18 && \
  make -j3 && make install)
cargo build -p bennett
front/test.sh
cargo test -p bennett-front-tests --test sigjmp_copy copied_sigjmp_buffers -- --ignored --nocapture
# Equivalent direct runner; --musl-cc/--libdir/--front/--executor/--out override locations:
python3 rt/musl/test-sigjmp.py
```

Measured on x86-64 with clang 18.1.3, glibc 2.39 and native musl 1.2.5: at both
`-O0` and `-O2`, all 576 device cases agree with glibc stdout/status.
Native musl differs from glibc in 94 cases at O0 and 92 at O2; the device follows
glibc in all of them. Native musl also changes 56 rows between optimization levels:
its saved-register reference to the original buffer can change both the landing
site and mask after that original is re-saved. Musl's result is measured separately
at each level, never substituted for the decided reference. Full native output,
device output and per-case differences are generated under `target/front-tests/sigjmp-copy`.

One defect remains: `sigjmp_partial_record.c` copies 16 bytes between two live
savemask-0 environments. This changes callee-saved-register bytes natively, leaving
the copy's continuation intact (glibc and musl status 0, empty stdout); it instead
replaces the device's continuation (status 1, empty stdout), at O0 and O2. The
ignored Rust witness is marked `defect:` and was explicitly observed failing:

```sh
cargo test -p bennett-front-tests --test sigjmp_copy a_partial_record_copy -- --ignored --nocapture
# Or: python3 rt/musl/test-sigjmp.py --partial-record
```

Root cause: the device stores the continuation in bytes 8..16 of its 32-byte
record; x86-64 glibc stores its saved instruction pointer in bytes 56..64. The
passing matrix's partial record copies end at bytes 64 or 72 and therefore do not
exercise this disagreement. Route: preserve glibc's byte-level copy semantics in
the lowered C environment while keeping the device instruction's layout separate,
then enable the witness and extend the prefix/suffix boundary sweep. Jumps after
a saving frame returned remain excluded, as decided.
