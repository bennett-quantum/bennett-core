# HANDOFF

State on 2026-11-15, evening: work resumed at about 18:45 UTC with jobs in flight on the musl runtime (from
`wip/rt-musl`) and its independent testing pass, the journal-content measurement, the testing pass of `exit`, the
console call and constructors, and the salvage survey. Code on `main` is unchanged since `4dfb423`.

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
| `f51861b`..`40e7e26` | `experiments/libc-probe`: musl 1.2.5 and picolibc 1.8.12 as bitcode under the device, output and status compared with native |
| `60bd56b`, `2f11a5f`, `bc8661c` | The console reachable from C; the `exit` instruction; constructors and destructors run, or their linker arrays supplied (a silent wrong result the probe found) |
| `9e15ebf`, `4dfb423` | Large data strings read without recursion (the debug CLI overflowed its stack); musl 1.2.5 vendored unmodified (`rt/musl`), its 1,345 C files built to bitcode offline |

Check it: `cargo test` (71 tests, about 2 s, no LLVM needed) and `front/test.sh` (98 tests, about 30 s with a clean build; needs LLVM 18, clang 18, CMake). `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are clean.

On GCC's C torture tests 1,521 of 1,576 runnable programs agree with native, 53 are refused by name, none is wrong;
on its floating-point tests (`ieee/`) 58 of 67, 9 refused (C library calls), none wrong
(`experiments/c-corpus/README.md`). The corpus on `main` compares exit status only and links no C library; the
probe, with musl linked and output compared, had 1,438 GCC programs and 212 of 220 c-testsuite programs agreeing
before floating point landed (`experiments/libc-probe/README.md`). The two have not been measured together.

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

- **The musl product runtime** (next step 1), stopped at wind-down on 2026-11-15. On `main`: musl vendored and
  built to bitcode. On the branch `wip/rt-musl` (one commit, `2adbb26`, on top of `4dfb423`; the four checks pass
  there, but it is not validated on the corpora, so it is not on `main`): startup that enters musl's own
  `__libc_start_main`, the system-call function, `rt/musl/link.py` (links as a static linker pulls archive
  members), five integration tests (`printf` with floats, `strtod`, `sqrt`, `sin`, `pow`, nested `exit`, arena
  exhaustion, an unknown system call, standard error), and a corpus harness that compares standard output as
  well as status. Choices made there, open to change: musl is built with `-fno-rounding-math`, so `fesetround`
  has no effect (with the flag, musl's `sin` is refused at a constrained intrinsic); descriptor 1 is the
  console and a write to descriptor 2 is a named fault; the arena is a link parameter, 1 MiB by default, `brk`
  upward and anonymous `mmap` downward, `munmap` reclaims nothing; `argc` is 1, standard input and the
  environment are empty; files and every other system call trap. `hello` printed `hello 42` in 3,691 steps.
  Not done: an audit of the system-call corner cases; the corpora with the runtime linked (the baseline with
  the new harness is 1,521 / 52 refused / 0 wrong on GCC, 151 / 68 on c-testsuite, 58 / 9 on `ieee/`; no run
  with musl was started, so the prediction in next step 1 is untested); the refusal tables; the complex-number
  helpers. To resume: check out `wip/rt-musl`, rebase on `main`, finish the audit, run the three corpora from a
  clean commit with copied binaries, reduce every wrong result or crash, then land.
- **Control schedules on the SQLite traces** (`experiments/sqlite-footprint/schedule.py`). Unfinished and unchecked;
  one workload completed. The preliminary table and the open points are at the end of that directory's README.
- **Journal content.** Nothing was written. The task: on real coherent regions, classify every journal word as the
  same on every branch, recoverable from live state, or irreducible, under two alignments, by record kind, with
  slopes against step count; on everything that runs today plus memory-heavy lookups written in C (binary search,
  hash table, B-tree over 1,024 keys) and one workload that writes memory. It needs a measurement hook in the
  executor that changes nothing when off. It decides whether the two optimisations buy orders of magnitude or a
  constant.

- **Constructor startup, after its independent testing pass** (`40b04b4`, Muse; `exit` and the console call held
  under every probe). It found two wrong results, both fixed in `e13a6c8` and now regression tests: at equal
  priority the device ran explicit `.init_array`/`.fini_array` contributions after the special-array entries
  (the front now follows the order LLVM emits and GNU ld links, measured against native builds); and a reference
  to the array bounds from a function the optimiser deletes handed startup to C, so a constructor silently never
  ran (ownership is now decided on the references that survive the optimiser). Order at equal priority follows a
  native build of the linked module; separately compiled objects linked natively can differ (C leaves it
  unspecified; `docs/DESIGN.md`). Safe refusals noted by the pass: a constructor through an alias; a null entry
  in a section array; an array variable in `.init_array` with alignment above 8; `front/include/bennett.h`
  cannot be included from C++ (`_Bool`).

## Next steps, in order

1. **Finish the musl 1.2.5 runtime at the system-call boundary** decided above (state: "Stopped in flight").
   Console and whole-run exit are device calls; the front supplies constructor/destructor arrays or runs them around the entry when no C startup
   owns them. Compile musl to bitcode with the probe's overlays and generic C substitutions, implement the supported
   system calls with console effects and a fixed arena, and trap by name for the rest. Keep musl's startup and exit
   code responsible for constructors, destructors, atexit handlers and flushing.
   Predicted, not yet measured: it removes 44 of the 53 refusals on the GCC corpus (`printf`-family 29, `stdout`/`stderr` 5, `open` 2, `pow`,
   `__mulsc3`, `__divsc3`, `fopen`, `qsort`, `isprint`, `longjmp`, `_setjmp`), 66 of 68 on c-testsuite and all
   9 on `ieee/` (`signal` 3, `sqrt`, `sin`, `ilogb`, `ilogbl`, `sprintf`, `__divsc3`); floating-point programs
   that wait on it: 8 GCC, 2 c-testsuite, 5 `ieee/`.
2. **Finish the two stopped measurements.** The journal-content one should come before step 3: it decides
   whether keeping history under superposition costs a constant or orders of magnitude, which neither this
   repository nor the earlier workspace has measured (advisor's recommendation, 2026-11-15).
3. **Build the journalled machine in the executor** and run the experiments listed in `docs/DESIGN.md` as it is
   built: every journalled step is a permutation; `apply` equals mark, call, rewind-keep; un-running through gates
   equals the conjugated operation.
4. **The front's fallback**: the program almost verbatim under marks, the documented coherent class recognised by
   its form, and the two warnings (history still entangled at a measurement or at the end; an un-run that meets a
   state its history does not match).
5. **Floating point** is done over Berkeley SoftFloat 3e (decided 2026-11-15; `rt/softfloat/README.md`). Left:
   the complex multiply/divide helpers (`__mulsc3`, ...; compiler-rt vendored after musl lands), `fmal`
   (SoftFloat has no 80-bit fused multiply-add), conversions to and from 128-bit integers, `_Float16`,
   `__bf16`, `__float128`. Then `setjmp`/`longjmp` and the remaining intrinsics.
6. **A SQL query as an oracle.**

## What the earlier workspace offers

- **The salvage survey** (`docs/SALVAGE.md`, ordered by the next step that would
  use each piece, with what was re-run and what was only read). The one direct code carry is for step 1:
  `bennett-runtimes`' clock and entropy models, file system and system-call dispatch (about 550 lines of ordinary
  C) fill the traps of the musl runtime, once what time and randomness mean on the device is decided. For step 5,
  `bennett-arith`'s bitcode uses integers far wider than the device's 128 bits, so only its test oracles carry;
  Bennett.jl's branch-free routines remain the cheap port. For step 6 no lifted SQLite oracle ever existed; the
  measured findings and the acceptance shape carry. Steps 2 and 3 carry designs and test shapes, not code.

## Decisions the maintainer took on 2026-11-15: the two open choices

- **Zero-filled data in the text form: a compact `zero N` data item.** The text form is not frozen, the change is
  small, and the stack floor stays a stated fact instead of resting on a byte that looks meaningless. Built in
  `4e34a3c` (`docs/SYNTAX.md`, "Data items"): the front writes runs of 16 or more zero bytes as `zero N`;
  `Module::static_end` counts the extent. Measured against the front before the change: lowest stack address,
  memory size and initial image identical on 120 front-test programs and on every tenth GCC corpus program (154);
  the sample's text went from 648 MB to 2.3 MB, and a musl-linked `printf("hello %d\n", 42)` from 4.4 MB to
  214 kB, same output in the same 3,718 steps. Its independent testing pass (`60aafad`: 1,233 stack-floor
  comparisons, 135 generated globals at every run length 0..33, megabyte arrays, all against explicit zero bytes
  or native) found no wrong result; one wording mismatch in the grammar (`+0`) was corrected in `07fdea4`.
- **The complex-number helpers (`__mulsc3`, `__divsc3` and the double and long double versions): vendor
  compiler-rt's sources unmodified**, as SoftFloat was, after the musl runtime has landed so that any `logb` or
  `scalbn` they need comes from musl; checked bit for bit against native. Whether the helpers are self-contained
  or call the maths library for some widths is not yet known. Not built yet.

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
- The vendored musl archive was checked byte for byte against the copy in the earlier workspace; its checksum
  could not be compared with upstream's published one (the address did not answer).
- The numbers in `docs/AUDITION.md` are each proposal's own and were not recomputed.
- The implementers' mutation checks found blind spots more than once (a Hadamard-prepared state hid a missing
  negation). A passing suite here is evidence, not proof.

## How work has been done here

- One goal at a time, from the centre outward; correct first, cheap second.
- Measure before deciding; find the fact that could sink the idea early.
- An implementer builds; one independent testing pass looks for wrong answers; findings are fixed and become
  regression tests; no repeated review rounds.
- Real design decisions are put to several independent designers and compared before the maintainer decides.
- Nothing private goes into the tree.
- Since 2026-11-15 (maintainer: land the work and push regularly): each job works in its own checkout of this
  repository, runs `cargo test`, clippy, fmt and `front/test.sh`, rebases on `main` and pushes each finished step
  to `main`; unfinished work goes to a `wip/` branch, never to `main`. A persistent advisor holds the picture and
  launches one job per goal; launches stop when the quota's pace is reached.
