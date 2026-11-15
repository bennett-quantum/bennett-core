# HANDOFF

State on 2026-11-15, about 13:30 UTC, after the floating-point job (below). Nothing of it is running.

## What this repository is

The restart of Bennett in one workspace, grown from the thesis program outward: rotate a bit, let ordinary code
depend on it, measure. The earlier `bennett-*` repositories are a read-only archive; this is the only place work
happens for now, as a trial of this way of working. Vision: `docs/NorthStar.md`. Working design: `docs/DESIGN.md`.

## What works

C with a few marker calls goes through stock `clang` 18, then `bennett-front`, then the executor.

| Commit | What it added |
|---|---|
| `3c7465d` | The device program (`crates/ir`), the executor with forward, un-run and coherent regions (`crates/exec`), the CLI |
| `66659f2` | Measurement on native SQLite: worst case against typical, and accesses whose address differs between rows |
| `70cfd37` | `front/`: C to a device program; the thesis program end to end |
| `4b65247` | Five miscompilations found by one testing pass, fixed; region faults are reported by name |
| `f36928f` | 1,876 real C test programs through the pipeline against native |
| `9ca6db3` | Controlled gates and in-place updates in C; Grover over a plain C predicate on 8 bits |
| `b5ffb85` | Three more refusals by name (mismatched prototype, an LLVM optimiser crash, a bit a type lacks) |
| `8dfc289` | Variadic calls, 128-bit integers, struct and vector values, intrinsics, computed goto |
| `b9f504d`..`167bafe` | `float`, `double`, `long double` over Berkeley SoftFloat 3e (`rt/softfloat`), checked bit for bit against the hardware; GCC's `ieee/` tests |

Check it: `cargo test` (44 tests, about 2 s, no LLVM needed) and `front/test.sh` (79 tests, about 30 s with a
clean build; needs LLVM 18, clang 18, CMake). `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are clean.

On GCC's C torture tests 1,521 of 1,576 runnable programs agree with native, 53 are refused by name, none is wrong;
on its floating-point tests (`ieee/`) 58 of 67, 9 refused (C library calls), none wrong
(`experiments/c-corpus/README.md`). The corpus compares exit status only.

## Decisions the maintainer took on 2026-11-13

- **North Star iteration 8**: 26 passages reviewed, 13 kept, 10 reworded, 3 struck (`docs/NorthStar.md`).
- **Coverage must end at 100%.** No refusals in the end unless physics forbids it, and then the reason is stated.
  A "not handled yet" is a debt.
- **What is promised when history is left behind** (`docs/DESIGN.md`): classical results are exact; coherence is
  guaranteed for a documented class of updates and for anything un-run; elsewhere it is unspecified and the
  toolchain warns; un-running after something changed what the region read or wrote is a warning, not a refusal.
- **The next extension** (`docs/DESIGN.md`, "The next extension"): a journalled machine as meaning and fallback,
  `mark` and `rewind ... keep`, memory as an overlay over a classical base, gates that take addresses; `apply`,
  `qalloc` and `qfree` stay until the new instructions pass every existing test. Decided, not built.
- **Channels** (partial trace, measurement by effects, noise) belong to the finished Bennett. The first working
  version is unitaries and clean discard, and nothing built now may make channels impossible to add.
- **Floating point is a low concern** (superseded on 2026-11-15, below). **The refused C library calls are a
  concern** and come first.

## Decision the maintainer took on 2026-11-15: floating point

- **Floating point is Berkeley SoftFloat 3e** behind the compiler runtime-call interface (the libgcc names),
  swappable at link time, for `float`, `double` and `long double` (x87 80-bit): it covers f32 and the 80-bit
  format, it is a verified reference, and it needs no hand port. Bennett.jl's branch-free routines are kept as
  a later, cheaper alternative for the optimiser, checked bit for bit against it (`docs/DESIGN.md`).

## Decisions the maintainer took on 2026-11-15: the C runtime boundary

- **Console output is reachable from C** as `void __bennett_effect(char)`, lowered to the existing journalled
  `effect`. It is an ordinary device call, not a quantum marker.
- **`exit v` ends the run from anywhere**, with the same outcome as an entry return; it is journalled in reversible
  execution and a named fault inside a coherent region. C calls `_Noreturn void __bennett_exit(int)`.
- **The C library is musl 1.2.5 at the system-call boundary**: one function taking a system-call number, implemented
  by device effects, an arena and `exit`; everything not implemented traps by name. Chosen over picolibc for scope,
  because the goal is unmodified real programs. No design audition: the two libraries gave matching outcomes in the
  probe (`experiments/libc-probe/README.md`).

## Stopped in flight

- **Control schedules on the SQLite traces** (`experiments/sqlite-footprint/schedule.py`). Unfinished and unchecked;
  one workload completed. The preliminary table and the open points are at the end of that directory's README.
- **Journal content.** Nothing was written. The task: on real coherent regions, classify every journal word as the
  same on every branch, recoverable from live state, or irreducible, under two alignments, by record kind, with
  slopes against step count; on everything that runs today plus memory-heavy lookups written in C (binary search,
  hash table, B-tree over 1,024 keys) and one workload that writes memory. It needs a measurement hook in the
  executor that changes nothing when off. It decides whether the two optimisations buy orders of magnitude or a
  constant.

## Next steps, in order

1. **Build the musl 1.2.5 runtime at the system-call boundary** decided above. Console and whole-run exit are
   device calls; the front supplies constructor/destructor arrays or runs them around the entry when no C startup
   owns them. Compile musl to bitcode with the probe's overlays and generic C substitutions, implement the supported
   system calls with console effects and a fixed arena, and trap by name for the rest. Keep musl's startup and exit
   code responsible for constructors, destructors, atexit handlers and flushing. The product runtime is the next slice.
   It removes 44 of the 53 refusals on the GCC corpus (`printf`-family 29, `stdout`/`stderr` 5, `open` 2, `pow`,
   `__mulsc3`, `__divsc3`, `fopen`, `qsort`, `isprint`, `longjmp`, `_setjmp`), 66 of 68 on c-testsuite and all
   9 on `ieee/` (`signal` 3, `sqrt`, `sin`, `ilogb`, `ilogbl`, `sprintf`, `__divsc3`); floating-point programs
   that wait on it: 8 GCC, 2 c-testsuite, 5 `ieee/`.
2. **Finish the two stopped measurements.**
3. **Build the journalled machine in the executor** and run the experiments listed in `docs/DESIGN.md` as it is
   built: every journalled step is a permutation; `apply` equals mark, call, rewind-keep; un-running through gates
   equals the conjugated operation.
4. **The front's fallback**: the program almost verbatim under marks, the documented coherent class recognised by
   its form, and the two warnings (history still entangled at a measurement or at the end; an un-run that meets a
   state its history does not match).
5. **Floating point** is done over Berkeley SoftFloat 3e (decided 2026-11-15; `rt/softfloat/README.md`). Left:
   the complex multiply/divide helpers (`__mulsc3`, ...; need the C library's `logb`/`scalbn`), `fmal`
   (SoftFloat has no 80-bit fused multiply-add), conversions to and from 128-bit integers, `_Float16`,
   `__bf16`, `__float128`. Then `setjmp`/`longjmp` and the remaining intrinsics.
6. **A SQL query as an oracle.**

## What is still refused, and what removes it

| Refused today | What removes it |
|---|---|
| C library calls (and with them `sin`, `pow`, `printf("%f")`, `sqrt` under errno); `setjmp`; return address | Step 1 above |
| `fmal`; 128-bit integer conversions of floats; `_Float16`, `__bf16`, `__float128`; `#pragma STDC FENV_ACCESS` | Step 5 above |
| A store under qubit control; an update with no known inverse; a value needed after it changed; qubit variables in structs, heap objects or passed to functions; gates in a loop under qubit control | The journalled machine (step 3) |
| A qubit-dependent value used by ordinary code or printed; measurement under control; a dirty discard | Channels |
| Non-empty inline assembly; code without source | An emulator of the real machine, compiled as an ordinary program |

## Known weak spots

- The region budget counts device instructions, not the source-fixed unit the North Star asks for.
- A `_Bool` qubit costs 8 qubits.
- Some acceptance decisions in the front rest on one LLVM pass proving an identity; when it cannot, the front
  refuses, which is safe but brittle.
- Variadic calls follow the x86-64 System V convention and clang's inline `va_arg`.
- A function that uses `stackrestore` cannot run coherently yet.
- A null dereference does not fault.
- Floating point: which NaN comes back when several operands are NaNs is unspecified; `long double` encodings no
  arithmetic produces (pseudo-denormals, unnormals) differ from x87; a `volatile` vector is refused. Where C
  leaves a result open (`fmin` of two zeros, out-of-range conversions) the device follows the native x86-64
  program, so a different native target would disagree there.
- The numbers in `docs/AUDITION.md` are each proposal's own and were not recomputed.
- The implementers' mutation checks found blind spots more than once (a Hadamard-prepared state hid a missing
  negation). A passing suite here is evidence, not proof.

## How work has been done here

- One goal at a time, from the centre outward; correct first, cheap second.
- Measure before deciding; find the fact that could sink the idea early.
- An implementer builds; one independent testing pass looks for wrong answers; findings are fixed and become
  regression tests; no repeated review rounds.
- Real design decisions are put to several independent designers and compared before the maintainer decides.
- Nothing private goes into the tree. Commits and pushes happen when the maintainer says so.
