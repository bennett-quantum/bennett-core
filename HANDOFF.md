# HANDOFF

State on 2026-11-13, about 14:00 UTC. Work is paused at the maintainer's request. Nothing is running.

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

Check it: `cargo test` (44 tests, about 2 s, no LLVM needed) and `front/test.sh` (58 tests, about 26 s with the
build; needs LLVM 18, clang 18, CMake). `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are clean.

On GCC's C torture tests 1,402 of 1,576 runnable programs agree with native, 172 are refused by name, none is wrong
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
- **Floating point is a low concern**: the original Bennett.jl has soft-float versions of every LLVM floating-point
  operation, to be carried over. **The refused C library calls are a concern** and come first.

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

1. **The C library calls.** The 37 refusals on the GCC corpus are `printf`-family 25, `stdout`/`stderr` 5, `open` 2,
   `fopen`, `qsort`, `isprint`, `longjmp`, `_setjmp`; on c-testsuite 64 of 70 refusals are library calls. They are
   refused because the test harness supplies no C library (`experiments/c-corpus/rt.c` leaves stdio undefined), not
   because the compiler cannot lower them. The route is a real C library compiled to bitcode and linked in, with the
   system-call boundary mapped to device effects (console output, memory from a fixed arena). Not started. Choosing
   the library and the shape of that boundary is the first decision.
2. **Finish the two stopped measurements.**
3. **Build the journalled machine in the executor** and run the experiments listed in `docs/DESIGN.md` as it is
   built: every journalled step is a permutation; `apply` equals mark, call, rewind-keep; un-running through gates
   equals the conjugated operation.
4. **The front's fallback**: the program almost verbatim under marks, the documented coherent class recognised by
   its form, and the two warnings (history still entangled at a measurement or at the end; an un-run that meets a
   state its history does not match).
5. **Floating point**, by carrying over Bennett.jl's soft-float; then `setjmp`/`longjmp` and the remaining
   intrinsics.
6. **A SQL query as an oracle.**

## What is still refused, and what removes it

| Refused today | What removes it |
|---|---|
| Floating point; C library calls; `setjmp`; return address | Steps 1 and 5 above |
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
