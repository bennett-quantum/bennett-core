# HANDOFF

State on 2026-11-15, end of the evening session (work resumed at about 18:45 UTC; nothing is running). Landed:
the musl runtime, validated on the three corpora with no wrong result; `zero N`; the journal-content and
control-schedule measurements; the salvage survey; the floating-point leftovers (128-bit conversions, `fmal`,
`_Float16`, `__bf16`) and the complex helpers; two constructor-startup fixes; small aggregates returned as one
integer; two review-audit fixes; and an independent testing pass by another model family of each of these. The
last corpus run (`8bbb56c`, musl linked) has GCC 1,560 agree, c-testsuite 217, `ieee/` 64, none wrong.

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
| `e13a6c8` | Constructors and destructors in the order the native link gives them; startup ownership decided after the optimiser |
| `4e34a3c` | Zero-filled data written as `zero N` in the text form (a musl-linked hello: 4.4 MB to 216 kB) |
| `d8050df`..`b464c0b` | The C library: musl 1.2.5 at the system-call boundary, running its own startup and exit; signals a program sends itself; stdin at end of file; an honest `fenv` |
| `663e9fb`..`caaac7a` | Conversions between floats and 128-bit integers, `fmal`, `_Float16` and `__bf16`, each bit for bit against native (`rt/softfloat/README.md`) |
| `9817fe0`, `c17bde9`, `08d7799` | Measurements: journal content in coherent regions; control schedules on the SQLite traces |
| `cd2d920` | The complex multiply and divide helpers: compiler-rt 18.1.8 vendored unmodified (`rt/compiler-rt`), linked by the front where a program calls them; bit for bit with the same helpers on the hardware, NaN bits aside |

Check it: `cargo test` (95 tests, about 2 s, no LLVM needed) and `front/test.sh` (132 tests; ignored by default: the two heavy journal-content runs, an extended float sweep, a
musl-linked ABI check that needs `rt/musl/build.py` first, and the witness of a refusal that is a debt (a packed
129-bit bitfield struct; see "Known weak spots"), about 30 s with a clean build; needs LLVM 18, clang 18, CMake). `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are clean.

With musl linked, standard output and exit status compared with native (`experiments/c-corpus/README.md`):
GCC's C torture tests 1,560 agree, 9 refused by name, 6 named faults (programs that open files), none wrong;
c-testsuite 217 agree, 1 refused, 1 named fault, 1 over the step budget, none wrong; GCC's `ieee/` 64 agree, 3
refused, none wrong (`b961292`, after the complex helpers and the 128-bit conversions moved three GCC programs
from refused to agreeing). Without the library an earlier snapshot gives 1,521 / 52 refused, 151 / 68, 58 / 9. Since
the complex helpers, the two GCC programs that waited on them agree too, with and without the library (run
alone, not as a whole corpus): GCC 1,559 agree and 10 refused with musl, if nothing else moved.

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

- **The musl runtime: landed** (`d8050df`..`b464c0b`; `rt/musl/README.md`). musl 1.2.5's own startup and exit run
  on the device; its system calls are answered at one boundary function by console effects, an arena and `exit`;
  everything else traps by name. The audit, with each case run against native, found and fixed: `malloc` faulted
  on its first call (a `PROT_NONE` guard page), `abort` faulted (signals a program sends itself now run their
  handlers, before the raising call returns), stdin through `readv`, a front bug that made every `scanf` program
  fault at exit (a call through an unprototyped pointer to a non-variadic target), and `fesetround` reporting
  success while changing nothing (a silent wrong result; non-default floating-point modes and flags now stop
  with a named fault). An independent testing pass (sol) found eight boundary wrong results; all fixed, kept in
  `rt/musl/tests/regress/` (`rt/musl/test.py`, 22 programs; 23 with the complex test). Corpora: see "What works". Refusals removed against
  the prediction: GCC 40 of 52 (44 predicted; left: `__mulsc3`, `__divsc3`, `setjmp`/`longjmp`, whose musl
  versions are assembly), c-testsuite 67 of 68 (66 predicted; left: a program declaring its own `strlen`), `ieee/`
  6 of 9 (9 predicted; left: three complex-division checks, refused at musl's `ilogb` `#pragma STDC FENV_ACCESS`
  and needing the complex helpers). musl's startup and exit add about 2,050 steps. The corpora took 321 s, 49 s
  and 14 s with 6 runners on a 12-core laptop. Provisional choices unchanged (`-fno-rounding-math`; descriptor 2 a
  named fault, except that a zero-byte write succeeds; the 1 MiB arena; `argc` 1, empty input and environment).
- **Control schedules on the SQLite traces: measured** (`08d7799`; `experiments/sqlite-footprint/README.md`, all
  eight workloads, independently recomputed by `check_schedule.py`). What a superposed program counter costs in
  provided IR instructions against one mean run (a trace-derived model, not gates): exact structural alignment
  (call path, loop iteration, block; its key order is acyclic, so it is also a valid waiting schedule) costs
  1.07-1.18x for arith and like_name, 4.85x for like_note, 3.0-3.1x for scan; padding loops to their observed
  maxima costs 10-77x; every reached block at every step 366-2,940x. The greedy waiting schedule's 7.96x was the
  greedy heuristic, far from that optimum; the 2.04x in `docs/AUDITION.md` is not reproduced (its merge details
  are not recorded). Held-out halves of the input show learned bounds are not sound: exact structure misses
  75-333 held-out scan rows. Every family still depends on observed runs; what a compiler could build without
  them is tabulated there.
- **Journal content: measured** (`9817fe0`, `c17bde9`, depth-one recomputation in `e7a571c`; `experiments/journal-content/README.md`). A hook in the
  executor, off by default and shown to change nothing, classifies every journal word of a coherent region as the
  same on every branch, recoverable from live state, or irreducible, under two alignments. Answer for these
  workloads: keeping history under superposition costs a constant factor, not orders of magnitude. In every loop
  that repeats, irreducible words grow linearly with steps (0.41 to 0.86 per step against 1.23 to 1.36 journal
  words per step: a saving of 1.4 to 3.1 times). Over 256 branches and 1/2/4/8 chained lookups the irreducible
  share is 37/50/59/64 % for binary search, 36/46/51/53 % for a B-tree, 0/5.5/17/23 % for a hash table; of 67
  other coherent runs, 49 have none. It is an upper bound: only values that can be copied are credited, not
  values that could be recomputed. **Depth-one recomputation now measured** on the same workloads and both
  alignments: linear growth survives, at 0.840/0.388/0.711/0.781 irreducible words per step for binary search,
  hashing, B-tree and stack writes (copy-only: 0.862/0.405/0.749/0.810). Re-evaluating the old value's producer
  from live copies of its operands removes 199 Slot positions and 42 Jump parameters across the lookups; no
  additional Store positions. Copy-only classification remains available. Longer chains, replay, the time price,
  and global-memory writes inside a region remain unmeasured (the last is refused until the journalled machine
  exists). The independent check
  (`5d41748`, sol) reproduced every table and showed the hook off, the hook on and the executor before the hook
  give identical journals, outcomes and step counts on 127 tests; it found the classifier could lose a common
  live copy (unsorted lists under a binary search; no published number changes) and one slope rounded twice;
  both fixed in `613b925`.

- **The complex helpers: landed** (`rt/compiler-rt/README.md`, `VENDOR.md`). compiler-rt 18.1.8's `__mulsc3`,
  `__divsc3`, `__muldc3`, `__divdc3`, `__mulxc3`, `__divxc3`, vendored unmodified, compiled to
  `complex-runtime.bc`, linked by the front before floating point is lowered, where the program calls them
  without defining them (`--complex-runtime` replaces them). The `float` and `double` helpers and `__mulxc3` are
  self-contained; `__divxc3` calls the C library's `logbl` and `scalbnl`, and musl's `logbl` calls `ilogbl`,
  whose `FENV_ACCESS` pragma the front refuses: `__divxc3` stays refused by name (a debt, removed with the
  constrained operations). 491,520 directed and random tuples on the device against the helpers compiled
  natively: every non-NaN result identical; NaN results differ only in sign and payload, which LLVM leaves
  open. Corpora (only the programs that waited): GCC `20070614-1` and `complex-5` now agree, with musl and
  without; `ieee/` `cdivchkf`, `cdivchkd`, `cdivchkld` are still refused, at their own `ilogb`/`ilogbl` (musl's
  `FENV_ACCESS`). Found: those three **abort natively when built with clang's own runtime**
  (`--rtlib=compiler-rt`) and pass with libgcc, whose division is a different algorithm (it differs from
  compiler-rt's on 12-17 % of the division tuples, last bit and sign of zeros); on the device with a test-only
  `ilogb` they abort as well. See the question below.
- **Constructor startup, after its independent testing pass** (`40b04b4`, Muse; `exit` and the console call held
  under every probe). It found two wrong results, both fixed in `e13a6c8` and now regression tests: at equal
  priority the device ran explicit `.init_array`/`.fini_array` contributions after the special-array entries
  (the front now follows the order LLVM emits and GNU ld links, measured against native builds); and a reference
  to the array bounds from a function the optimiser deletes handed startup to C, so a constructor silently never
  ran (ownership is now decided on the references that survive the optimiser). Order at equal priority follows a
  native build of the linked module; separately compiled objects linked natively can differ (C leaves it
  unspecified; `docs/DESIGN.md`). Safe refusals noted by the pass: a constructor through an alias; a null entry
  in a section array; an array variable in `.init_array` with alignment above 8; `front/include/bennett.h`
  cannot be included from C++ (`_Bool`). The independent check of the fix (`9d40852`, sol) found it introduced
  a silent wrong result: when the optimiser folds a live read of `__init_array_start[0]` (or a fini bound) into a
  direct call, no bound reference survives, so the front wraps the entry as if C did not own startup and
  constructors or destructors run twice (`iim` and 2 where native gives `im` and 1). Fixed in `713d996`: the
  bounds stay undefined through the optimiser, as in a native compile, and are defined afterwards as a linker
  would, so a surviving reference reliably means C owns startup (`docs/DESIGN.md`). The witnesses are regression
  tests, with musl's own startup loops as one of them; the three corpora with musl linked are unchanged and musl
  programs take the same steps.

## Next steps, in order

1. **The musl runtime is done** ("Stopped in flight"). Left on it: files and clocks (`docs/SALVAGE.md` names the
   earlier workspace's device models; what time and randomness mean on the device must be decided first),
   `setjmp`/`longjmp`, and the two questions below.
2. **The two stopped measurements are done** ("Stopped in flight"). Journal content: a constant factor as far as
   copies and depth-one recomputation go; longer chains and their time price are unmeasured. Control schedules:
   exact structure costs about 1-5x a run on these traces, but only from observed runs.
3. **Build the journalled machine in the executor** and run the experiments listed in `docs/DESIGN.md` as it is
   built: every journalled step is a permutation; `apply` equals mark, call, rewind-keep; un-running through gates
   equals the conjugated operation.
   In progress since 2026-11-15, 21:17 UTC. The independent oracle for these experiments landed first
   (`2201a6e`, sol, written without reading the machine's code): `crates/exec/tests/independent_oracles.rs`, a
   dense matrix calculation (nalgebra, test-only) of gates, controls, `apply` regions and un-running through gates,
   and a brute-force bijection checker over whole finite states. Against today's executor: all six examples, 480
   gate/control runs, 64 conjugation columns, 32 generated circuits, 20,480 finite `apply` states and 512
   journal-prefix round trips agree; four deliberate executor mutations (a phase sign, a missing adjoint on
   un-run, reversed control polarity, an `apply` that assigns instead of XORs) each fail it. `mark` and
   `rewind keep` plug in through the checker once the executor exposes whole machine and history state
   (`crates/exec/tests/oracle_support/mod.rs`).
4. **The front's fallback**: the program almost verbatim under marks, the documented coherent class recognised by
   its form, and the two warnings (history still entangled at a measurement or at the end; an un-run that meets a
   state its history does not match).
5. **Floating point** is done over Berkeley SoftFloat 3e (decided 2026-11-15; `rt/softfloat/README.md`), the
   complex helpers over compiler-rt (`rt/compiler-rt/README.md`). Left: `__divxc3` (musl's `ilogbl`, under
   `FENV_ACCESS`) and `__float128`.
   Done on 2026-11-15: conversions to and from 128-bit integers (and `_BitInt` up to 128), `fmal` (over
   SoftFloat's primitives, rounding once), `_Float16` and `__bf16` (computed in `float` as the x86-64 back end
   does). Checked: every 16-bit input, all 2^32 `float` inputs of the conversions from `float`, 10^7 random
   operands per operation and 10^8 `fmal` triples against libgcc and glibc, 0 mismatches. The independent
   testing pass (`6873b1c`, sol) found no wrong number (123,368 `fmal` cases, 17,676 conversions, 46,640 device
   words; `ieee/` unchanged) but a cost regression from `a9ce585`, which always inlines runtime routines that
   return an aggregate: three `long double` to `__int128` conversions grow from 966 to 9,981 instructions and
   overflow a 64 KiB stack, and sixteen do not finish lowering in 120 s. Ignored witness in
   `front/tests/tests/float_independent.rs`. Fixed in `6418e6b` (fresh names without
   rescanning: lowering time linear in a function's size, output byte-identical) and `8bbb56c` (an aggregate of at
   most 128 bits of integer and float fields is returned as one integer, so the inlining is gone): 1/4/16/64
   conversion sites now take 879/897/969/1,257 lines (932/980/1,148/1,820 before `a9ce585`) and lower in about
   0.05 s; a controlled `fmal` at `-O2` lowers with no memory writes. Its testing pass (`8cea574`, sol: 4,224
   result words across layouts and separately optimised callers and callees, coherent and with musl) found no
   wrong answer. `powi` on the two
   short formats is refused by name (it calls `powf` natively). Then `setjmp`/`longjmp` and the remaining intrinsics.
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
  `scalbn` they need comes from musl; checked bit for bit against native. Built in `cd2d920`: self-contained except the `long double` divide,
  which calls `logbl` and `scalbnl` (and through musl's `ilogbl` a `FENV_ACCESS` pragma, so it stays refused).

## Questions for the maintainer (2026-11-15, evening)

- **Complex division: compiler-rt or libgcc?** compiler-rt's division (decided) fails GCC's three `cdivchk`
  accuracy checks natively, which pass with libgcc, the runtime of the corpus's native reference. When the
  `FENV_ACCESS` refusal goes they will be counted as wrong results. Keep compiler-rt and compare those against
  a `--rtlib=compiler-rt` native build, or vendor libgcc's division (GPL with the GCC runtime exception), or
  something else? (`rt/compiler-rt/README.md`, "Not libgcc".)

- **Floating-point environment**: keep the named fault for non-default rounding modes and flags, or make them real
  through SoftFloat's state? (The float runtime lets the optimiser delete its flags today.)
- **Exit status**: the device reports the full 32-bit `exit` value; a process status is its low 8 bits. The corpus
  harness compares modulo 256, the integration tests exactly. Mask at the boundary?
- **Time and randomness on the device** (clock start value, entropy image): needed before the earlier workspace's
  clock and entropy models can be carried over (`docs/SALVAGE.md`).
- **Equal-priority constructor order across separately compiled objects** follows a native build of the linked
  module, not separately linked objects (C leaves it unspecified). Record as a debt, with the route of lowering
  startup per unit before linking?

## What is still refused, and what removes it

| Refused today | What removes it |
|---|---|
| Files, clocks and the other unimplemented system calls (named faults); `setjmp`/`longjmp`; return address; a program that redeclares a library function incompatibly | Step 1 above |
| `__float128`; `powi` on `_Float16`/`__bf16`; `#pragma STDC FENV_ACCESS` (with it musl's `ilogb`, and so `__divxc3`, the `long double` complex division); non-default rounding modes and floating-point flags | Step 5 above |
| A store under qubit control; an update with no known inverse; a value needed after it changed; qubit variables in structs, heap objects or passed to functions; gates in a loop under qubit control | The journalled machine (step 3) |
| A qubit-dependent value used by ordinary code or printed; measurement under control; a dirty discard | Channels |
| Non-empty inline assembly; code without source | An emulator of the real machine, compiled as an ordinary program |

## Known weak spots

- Since `8bbb56c` some `long double` programs take more steps than before `a9ce585` (GCC `960215-1` 3,863 to
  4,052, `regstack-1` 4,145 to 4,354, c-testsuite `00204` 1,068,923 to 1,089,594; 25 other programs got
  cheaper). The cause is not measured.
- A small aggregate result that contains a pointer is still returned through memory, so it is refused under qubit
  control at `-O2` (route: pack it too, after measuring what turning a pointer into an integer costs LLVM's alias
  analysis). A packed struct of 129 bits of bitfields becomes an `i136` and is refused (route: split memory
  operations LLVM widens past 128 bits before emission).

- The region budget counts device instructions, not the source-fixed unit the North Star asks for.
- A `_Bool` qubit costs 8 qubits.
- Some acceptance decisions in the front rest on one LLVM pass proving an identity; when it cannot, the front
  refuses, which is safe but brittle.
- Variadic calls follow the x86-64 System V convention and clang's inline `va_arg`.
- A function that uses `stackrestore` cannot run coherently yet.
- A null dereference does not fault.
- `bennett run --arg` refuses arguments for parameters wider than 64 bits (`Executor::new` takes `u64`).
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
