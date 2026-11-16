# HANDOFF

State on 2026-11-16, 03:20 UTC, end of the night session (nothing is running). Landed overnight: **the journalled
machine in the executor, all five steps of its order** (`mark`/`rewind keep`, worlds, memory as an overlay, gates
that take addresses, `apply` computed as mark, call, rewind-keep), each with its experiments and an independent
testing pass by another model family, and every wrong result those passes found fixed as a regression test (next
step 3); two independent oracles for it (a dense matrix calculation and a whole-world reference model); the
recompute witness and recovery chains for journal content, a completeness gap in them found by an independent
check and fixed, and journal content with global-memory writes (next step 2); `setjmp`/`longjmp`, return
addresses, over-aligned variadic arguments and calls through another prototype in the front, each with its testing
pass, and a silent wrong result already on `main` found on the way (by-value arguments and clang's `tail` mark).
The last corpus run (`64571ac`, musl linked, 2026-11-16) has GCC 1,569 agree, c-testsuite 218, `ieee/` 64, none
wrong or crashing. New questions for the maintainer are in "Questions for the maintainer" (worlds W1-W5, qubits in
memory M1-M3, `apply` A1-A3, non-local jumps, return addresses, pointer-call cost); the earlier ones are still open.
Earlier: the evening session of 2026-11-15 (the musl runtime, `zero N`, the floating-point leftovers, the complex
helpers, constructor startup, the first journal-content and control-schedule measurements, the salvage survey).

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
| `31f5138` | `setjmp`/`longjmp` (library and built-in forms, `sigsetjmp` with musl): a device `setjmp` instruction and `longjmp` terminator, journalled (`docs/DESIGN.md`, "Non-local jumps") |
| `2bf0b69` | Return addresses: `%r = retaddr N` reads the device's frames (`docs/DESIGN.md`, "Return addresses"); GCC `20030323-1`, `20030811-1`, `pr17377` agree with native, with musl and without |
| `595339c` | `sigsetjmp`'s signal mask kept in the jump environment: the two wrong results of its testing pass fixed, checked against native on every small way to save, copy and jump |

Check it: `cargo test` (279 tests; ignored: heavy reference, generated or hash-sample runs, the memory-cost report, 2 heavy recomputation checks, the scratch mutation matrix, the canceled memory keep refusal witness; about 9 s, no LLVM needed) and `front/test.sh` (158 tests; ignored by default: a musl-linked return-address check, the three heavy journal-content runs, two heavy global-write runs and the global-write check's image writer, an extended float sweep, a
musl-linked ABI check and two independent variadic checks that need `rt/musl/build.py` first, and the witness of a refusal that is a debt (a packed
129-bit bitfield struct; see "Known weak spots"); needs LLVM 18, clang 18, CMake). `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are clean.

With musl linked, standard output and exit status compared with native (`experiments/c-corpus/README.md`):
GCC's C torture tests **1,569 agree, 1 refused by name, 5 named faults**, 81 skipped;
c-testsuite **218 agree, 1 named fault, 1 over the step budget**;
GCC's `ieee/` **64 agree, 3 refused**, 1 skipped; **none wrong or crashing** (`64571ac`, 2026-11-16).
All 1,944 programs compared per program with `eeff78d`: only GCC `pr67037` (`bad_indirect` → agree,
2,139 steps) and c-testsuite `00025` (mismatched `strlen` refusal → agree, 2,084 steps) change outcome,
both from the new prototype-call lowering. Every other outcome and normalized reason is unchanged.
On 1,856 unchanged-outcome rows with steps: **1,855 increase, none decrease, one unchanged** (8-queens
still at the 10^8-step budget). Median extra steps / relative increase: GCC **43 / 2.0408%**, c-testsuite
**30 / 1.4648%**, `ieee/` **43 / 1.8150%**. Largest relative: GCC `bcp-1`, 2,118 → 2,580 (**21.8130%**).
Measured cause: the prototype-call pass's checks, including musl startup/exit dispatch. In `bcp-1`,
the same 14 table calls add 238 instructions in `main` and 224 in runtime dispatch; largest absolute
increase is c-testsuite `00186`, +846 (740 in `vfprintf`). Old-front/current-executor replay reproduces
all 1,944 old outcomes/reasons/steps exactly; paired site counters account for every numeric step delta.
`00025`'s accepted `main` folds to `ret 0`; it is compilation coverage, not an executed `strlen` check.
Three workers maximum, musl linked, 1 MiB stack, 10^8-step budget; wall times GCC 602.85 s, c-testsuite
80.52 s, `ieee/` 29.32 s. Report and each moved case: `experiments/c-corpus/README.md`; ignored results,
frozen binaries, complete comparisons and per-program step attribution: `.tmp/corpus-2/results-64571ac/`
in the testing checkout; paired old/new site profiles: `.tmp/corpus-2/replay-eeff78d/`. Without the library
an earlier snapshot gives 1,521 / 52 refused, 151 / 68, 58 / 9.

Independent testing of `cee23f8` and `eeff78d` (`front_call_independent.rs`): 3,240 exact native/device by-value
words over 27 layouts and direct/indirect/sibling/recursive/local-copy mutation paths; 720 mixed-vararg words
across baseline/AVX2/AVX512 and 240 more at 16,384-byte variadic alignment; 96 controlled branch values and
amplitudes; 480 musl-printed words and 12 forwarded `vprintf`/`vsnprintf` stdout/status comparisons.
`-O0`, `-O2`, `--taint-all`; all valid power-of-two by-value alignments through 16,384. No new wrong result;
`musttail` with a by-value argument is refused by name. The new test catches the old tail bug against the saved
`8bbb56c` front (17-byte struct at `-O2`: 0 versus native 6,046,364,335,294,611,224), and passes at `eeff78d`.
AVX512F is unavailable on this CPU: device AVX-512 lowering is compared with the same C executed natively without
AVX-512 instructions; native AVX2 is also executed. The two musl tests are ignored by default and were run
explicitly after the separate musl build.

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

- **SQLite on the device, all three stages measured** (`experiments/sqlite-on-device/README.md`): unmodified 3.54.1,
  musl linked, OS_OTHER no-files VFS, all four footprint query shapes at N=4/8/16 agree with native on
  stdout/status and all 112 fresh-setup results. Full runs 957,295/1,200,929/1,841,474 steps;
  device text about 9.06 MB, final lowering 19.50–24.09 s. SQLite live/peak 101,632/107,264 bytes;
  256 KiB SQLite heap, 4 KiB libc arena, N=4 stack minimum 155,099 bytes (one less: `stack_overflow`).
  Full reversal restores the entry snapshot exactly at all three N; journals 19,086,335/23,956,769/
  36,573,538 bytes, 1.184/1.184/1.179 u128 words per step. No wrong result or crash.
  N=4 `like_note` under hand-written `apply` agrees with native on every basis and in superposition:
  7 qubits, 4 configurations, fault zero; 36,056 region steps, 72,116 with uncomputation, largest
  per-world journal/overlay 215,617/416 bytes. C markers still refuse the memory-writing query call;
  no compiler/runtime changes. The 11,048-step region budget passes, one less faults only row 3.
  Grover with the unchanged LIKE query passes N=4 for every singleton, two matches and none,
  and N=8 for one match at k=1/2 (success 25/32, 121/128) and two at k=1, amplitude error
  at most 7.78e-16 (two matches at N=4 stay at success 1/2, including k=1/2/3).
  Repeated N=4 two-match / N=8 singleton calls cost 76,886 / 148,070 region steps and
  753,471 / 1,453,019 journal bytes each, retaining no journal/overlay or extra qubits;
  native and independent apply comparison agree.

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
  additional Store positions. Copy-only and depth-one classification remain available, with all 428 old data
  rows reproduced exactly. **Unbounded producer chains and forward instruction time now measured**, on all
  83 workloads and both alignments. Corrected live-state DAG slopes are 0.848/0.341/0.606/0.641; adding
  earlier already counted classical/retained journal words gives 0.308/0.033/0.331/0.347. At 8 reps that costs
  1506782/362752/1020248/572976 re-evaluated instructions across 256 branches, averaging
  9.279/18.645/4.896/10.131 per recovered word. This is a greedy local tape with one common producer/source
  circuit per position, not optimal replay. After fixing the independent check's completeness gap, all corrected
  lookup counts and slopes agree with its greedy rerun, both alignments. Raw step words keep chain context even when an
  instance has a first-write, unchanged or first-store witness; a common circuit can still recover the
  whole position. Copy-only and depth-one are unchanged. At reps 8 tape retains 322/7/725/212 positions;
  282/0/622/150 have individual recipes. Binary/B-tree's other 40/103 words need control-presence replay;
  all seven hash residuals hit missing `%call` origins. Writes also have changed-memory, missing-call and
  unavailable whole-word origins. These corrected diagnostics replace the inflated counts that mixed
  missing context with real producer restrictions. Linear growth survives the measured common-circuit
  witness, but unavoidable linear information loss is not established. Guarded producer/call replay,
  global pebbling and DAG scratch/cleanup cost remain open. Global-memory writes are measured below. Tables and exact rules:
  `experiments/journal-content/README.md`, `chains.tsv`. All 49 zero-copy-Irreducible other runs stay so;
  46 are entirely Same, three also use copies (corrected from the older prose using the unchanged tables).
  **Global writes measured on the worlds machine** (`front/tests/tests/journal_global.rs`,
  `experiments/journal-content/global.tsv`): ordinary lowered C log/hash/B-tree updates plus the four existing
  lookups, 256 inputs, 1/2/4/8 reps, all four classifiers and both alignments. The hook still follows only
  `apply`: its callee journal is the explicitly qualified proxy; real mark/call/rewind driver journal,
  overlay and rewind costs use Stats. Outer mark/read/root-return/rewind history is not classified;
  per-world provenance/collector forking and capture at rewind barriers is the route. At 8 reps log tape I=0
  (copy I=56), hash=91, B-tree A/B=3344/1167; fitted tape I/step 0/.407/.992(A) or .295(B).
  Global writes alone do not imply a growing retained tape; the positive residuals are common-circuit
  witness limits, not information lower bounds. Actual 256-world peak journal/overlay bytes at 8 reps:
  log 818688/5976, hash 1159024/11374, tree 12503757/6188. Rewind work 39424/65317/941736 steps, no reruns.
  A late-split eight-configuration sweep compares copied overlays/journals at three classical memory sizes; counters
  are independent of extra memory. Native results, final apply/world amplitudes and memory, full reversal,
  off/on state/Stats and byte-retention checks pass. Old lookup C lowers byte-identically and all 1428
  published chain/tape rows reproduce exactly; old tables/classifiers are unchanged.
  Its independent check (`cce2bcc`, Opus) reproduced every number byte for byte, found every credited word sound and
  none missed on all 28 runs, and found the `apply` callee journal record for record the worlds' own journal (words
  differ only by a constant stack offset per kind). It qualifies the conclusion: it rests on the log, whose writes
  destroy nothing branch-dependent; where global writes destroy branch-dependent bytes the residual stays (hash:
  12 of 16 store positions irreducible; B-tree A: 9 of 57 copy-only, 5 with the tape).
  Heavy chain measurements are ignored by default. After integrating the observer with `setjmp`/`longjmp`
  on base `ff338cb`, all 1,992 then-published data rows reproduced exactly; the later completeness fix
  changes only the chain/tape rows (now 1,428), with all 428 copy/depth-one rows unchanged. Jump
  words have distinct indexed roles; all 32 bytes of each `setjmp` write are tracked for current-memory
  checks, and `longjmp` control words remain opaque to scalar aliases. Executor regressions cover these
  rules and raw/counts-only observer state/statistics equivalence. The independent check of the original copy observer
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

- **`setjmp`/`longjmp`: landed** (`31f5138`; `docs/DESIGN.md`, "Non-local jumps"; `docs/SYNTAX.md`). The
  device has no registers, so not musl's assembly: `%r = setjmp %buf` writes a 32-byte jump record (frame
  depth and function, position, entry and current stack pointer) into program memory and gives 0; the
  terminator `longjmp %buf, %v` checks the record against the live frames (otherwise the named fault
  `bad_indirect`, shared with `call_indirect`: the 4-bit code space is full), discards the frames above, restores the stack pointer and resumes after the
  `setjmp` with `%v`. Both are journalled: un-running through jumps restores the state exactly, checked
  step by step, and by a `rewind` (a jump counts as the returns of the frames it discards when a rewind
  checks its mark's activation). In a coherent region a jump within the region works on every branch; out
  of it, `bad_indirect`. The front lowers the C library's names (`setjmp`, `_setjmp`, `longjmp`, `_longjmp`,
  `siglongjmp`, `__longjmp_chk`; `sigsetjmp` saving the mask through musl's own `__sigsetjmp_tail`, as
  musl's assembly does), clang's `__builtin_setjmp`/`__builtin_longjmp` and `llvm.frameaddress(0)`.
  Executor changes (rebased onto `mark`/`rewind`, `0513c5e`): `Undo::Setjmp`/`Undo::Longjmp` and their
  forward and backward steps (`forward.rs`, `backward.rs`), the journal's list of discarded-frame counts
  and its use in `open_mark` (`journal.rs`, `history.rs`), the journal-content observer following them
  (`measure.rs`), and one panic arm for `longjmp` in the oracle's interpreter (`oracle_support/dense.rs`).
  Measured: GCC `20210505-1`, `pr56982`, `built-in-setjmp`, `pr84521` agree with native, with musl
  (2,059 / 2,074 / 2,161 / 2,122 steps) and without the library (10 / 33 / 130 / 73); ten front functions
  (nested, repeated, across frames, from a callback, through a copied `jmp_buf`, value 0, the built-ins,
  allocations freed by the jump) on 32 inputs each against native from -O0 and -O2 and with `--taint-all`;
  `rt/musl/tests/setjmp.c` (masks saved and not, jumps out of signal handlers) prints what native prints.
  Its independent testing pass (`e8149f1`, sol: 512 device runs matched to native with exact reversal,
  recursive and copied buffers, VLAs, the built-ins, eight coherent branches, rewinds, jumps out of signal
  handlers) found two wrong results with `sigsetjmp` (native status 0, device 1, at -O0 and -O2): revisiting a
  `sigsetjmp` lost a copied buffer's run-time `savemask`, and revisiting one site restored the last buffer's mask,
  not the target's. **Fixed** (`595339c`): the front took the buffer and `savemask` from the frame's slots after
  the jump, which hold the site's latest visit; now the mask state lives in the buffer (a flag word and the saved
  mask, written before the device `setjmp`) and every library `longjmp` restores its own target's mask before it
  jumps, as glibc does. Front only, no executor change. The witnesses are regular tests
  (`rt/musl/tests/regress/setjmp-saved-{mask,buffer}.c`); `rt/musl/tests/setjmp-masks.c` checks every way to save,
  copy and jump with two buffers, a copy, five call sites, two visits and two jumps (17,640 jumps), and a chain of
  sixteen buffers, against native: 65 of 421 output lines differed before, none now, from -O0 and -O2.
  `rt/musl/test.py` also links the `setjmp` programs from -O2 and un-runs the small ones (33 runs, about 30 s), and
  an executor test runs the lowered shape under a `rewind`, step by step.
  Debt: a function that uses `setjmp` is not run coherently from C (refused by name at a qubit-dependent
  call), because LLVM must see `setjmp` as writing any memory; route in `docs/DESIGN.md`.

## Next steps, in order

1. **The musl runtime is done** ("Stopped in flight"). Left on it: files and clocks (`docs/SALVAGE.md` names the
   earlier workspace's device models; what time and randomness mean on the device must be decided first)
   and the two questions below. `setjmp`/`longjmp` are done (`31f5138`).
2. **The two stopped measurements are done** ("Stopped in flight"). Journal content: a constant factor as far as
   copies, depth-one and unbounded common-producer chains go. The journal-assisted fitted slopes are
   0.308/0.033/0.331/0.347 words/step, with measured forward recomputation time. Remaining counts include
   producer-alignment restrictions, so this is not evidence of unavoidable linear information loss; guarded
   producer/call replay and global pebbling/cleanup costs remain open. Control schedules:
   exact structure costs about 1-5x a run on these traces, but only from observed runs.
   Independent check of both recomputation measurements (`b1454ec`, Opus) found every credited word
   sound and one completeness gap: a branch's cheap copy witness suppressed its chain context, so a
   common circuit was never tried for mixed-witness positions. **Fixed and remeasured** (`a74b3de`): context is now
   retained for every step word in raw mode, and the formerly ignored mixed-copy witness runs normally
   with both alignment keys checked. The corrected live slopes are 0.848/0.341/0.606/0.641 and tape slopes
   0.308/0.033/0.331/0.347 (binary search/hash/B-tree/writes), exactly the checker's independent rerun.
   All 428 copy/depth-one rows are unchanged. The fresh independent hand/random/C-lookup replay has no
   unsound credits or missed recoveries; the hook on/off step-by-step state and journal checks pass.
   README and `chains.tsv` now report the corrected retained tape, time costs and measured residuals.
   Linear growth remains a property of this common-circuit witness, not a lower bound on information loss.
   Global-write extension: log/hash/B-tree insertion measured with hand-written worlds and `apply` drivers;
   log tape residual is zero, hash/tree positive (A/B matter for recursive insertion). Actual world journal
   payload is much larger than overlay payload on these runs. See the journal-content README/global.tsv;
   classification of outer worlds/read/rewind history remains a documented observer gap with an explicit route.
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
   **First step landed** (`d1db45c`): `%h = mark` and `rewind %h keep (v -> %reg, ...)` in the text form
   (`docs/SYNTAX.md`), the validator and the executor, on today's single classical state (`docs/DESIGN.md`).
   A rewind XORs the kept values in, un-runs every step since the mark (gates inverted, so `mark; h q; rewind keep
   (i1 1 -> q)` is Z) and continues; marks nest; a rewind that cannot un-run (stale or foreign handle, a
   measurement since the mark, a keep into a register allocated after the mark) is the named fault `bad_rewind`
   and changes nothing. Measured: every step is a permutation by brute force (38,360 single steps, 3,016 of them
   faulting, and 8,192 rewinds from their mark, on clean and dirty history); the known exponential is real (every
   level cleaning up: work 19, 55, 127, ... 36,847 at depth 0..10; keeping callees' history open until the outer
   rewind: 21 + 20d); a mark, call, rewind loop costs the same per iteration at 50 and 1,500 iterations.
   Its independent testing pass (`8c917c4`, sol: the oracle plugged in; 2,180 matrix comparisons of un-running
   through gates under `rewind keep`, 18,432 whole-state images of mark and nested rewinds on clean and dirty
   history, 64 stale-handle fault states, all agree) found two wrong results, both fixed (`d012728`): **F1** a
   rewind whose un-run freed a temporary entangled with a keep destination returned success with norm 0; now an
   un-run that meets a state its history does not describe (a register allocated since the mark not zero when
   its allocation is un-run, an inner rewind whose region faults when run again) runs the un-run steps forward
   again and faults `bad_rewind`, changing nothing, as the interim answer to question (1) below; checked
   against an independent calculation on 1,776 programs (`crates/exec/tests/history_unrun.rs`). **F2** undoing
   a rewind reached again in a loop replayed to its first visit; a rewind's record now keeps how many steps it
   un-ran and exactly that many run again. Costs of every other history test unchanged; rewinding one mark
   again doubles the work (n rewinds of one mark in a loop: 3 * 2^(n-1) - 2 steps un-run).
   A second independent oracle for the next step (`0ca2480`, sol, written from the documents without reading the
   worlds work): a whole-world reference model (`crates/exec/tests/oracle_support/worlds.rs`; a superposition is a
   map from whole classical machine states, memory as an overlay over a classical base, to amplitudes), agreeing
   with today's executor on 3,357 runs (28,123 instructions; every history, rewind and jump test program) and on
   1,280 whole-state bijection cases. Where worlds leave the documents silent it asks: how far collision
   detection must reach and which history a coherent fault keeps; whether a fault in one world stops all worlds;
   how marks, rewinds and replays align when worlds' histories differ in shape; what one measurement event is
   when worlds reach it at different places; whether dead slots count in world equality.
   **Second step landed: worlds** (`2c0563e`; `docs/DESIGN.md`, "Worlds"). A `call` may pass a register to an
   integer parameter; where configurations disagree on its value the machine state splits into one world per
   value (a full copy, memory included, journalled from then on); a rewind that makes worlds equal merges them and
   their amplitudes add. Worlds run in lockstep ticks (the budget counts ticks); a world that has rewound waits
   until no world runs, then equal worlds merge. Interim choices (questions below): a keep that changes a register
   the region read faults `bad_rewind` and changes nothing (also when a later gate carries it there); a fault in
   some worlds stops the run unless region faults are allowed; a measurement with several worlds waits for all,
   then observes which world; a run ending with several worlds hands the host one, sampled by weight, and
   reversal stops there. With one world the machine is the old one: every existing test passes, except one
   expected rewind word, now the visit number of the rewind rather than the steps it undid (paths of different
   length must merge). Measured: splits and merges are bijections by brute force (128 starting states each, all
   16 basis configurations and their superposition, clean and dirty history); `apply` equals mark, call,
   rewind-keep on the thesis program and Grover (16 seeds each), 108 basis states and superposed arguments (the
   idiom has no region budget: a debt); un-running through gates across worlds equals the independent dense
   calculation on 91 programs (75 equal, 16 refused exactly where the calculation says the read no longer
   matches), and the whole-world reference agrees on the same 91; the known exponential with worlds: cleaning up
   at every level c(d) = 2c(d-1) + 47 steps (22,481 at depth 8), history left open 48 per level (430 at depth 8);
   a loop of the idiom costs 31 steps and about 135 journal bytes per iteration at 50 and at 1,500 iterations.
   The reference model differs in two places, both where the documents are silent: it faults a register
   allocated, rotated and read inside one region (it checks each world's share for zero; the executor sums the
   shares and checks the norm when all worlds stop), and its lockstep gives no interference when worlds' paths
   differ in length.
   Its independent testing pass (`9b3fd45`, sol; the whole-world reference plugged into the executor): 256 generated
   coherent programs, 729 dense un-run cases (557 equal, 172 refusals exactly where the reads no longer match),
   2,048 whole-state split/merge images, 64 `apply`/idiom comparisons, sampling by weight at measurement and end
   of run (4,096 seeds each, within 0.035 per bin), 11,129 reference comparisons across every executor test. One
   wrong result: **a later split hid an earlier changed read**, so a rewind that must fault `bad_rewind` returned
   success with (q, out) = (1, 1). Fixed (`d6036a1`): below a world's split point the un-run's checks (a read
   still holds its value, an un-allocated or freed register is zero) are made on the sum of the worlds' parts once
   every world has rewound, and a failing sum puts each of those worlds back from a copy and faults `bad_rewind`
   (`docs/DESIGN.md`, "Worlds"). Checked on the whole class against the reference and an independent dense
   calculation: every order of three of reads, gates, a temporary and nested rewinds, 12,816 programs (1,710
   refused), and of four, 134,264, all agree; the previous executor missed 336 refusals and panicked on 15 of the
   12,816. Steps and journal unchanged; the 1,500-iteration idiom loop test takes about a third longer (the
   copy). The two earlier differences from the reference are decided by `docs/DESIGN.md` ("Worlds") in the
   executor's favour.
   **Third step: memory as an overlay** (`7c34bf5`; `docs/DESIGN.md`, "Memory as an overlay"; `crates/exec/src/memory.rs`).
   Memory is a classical base shared by every world plus, per world, a normal overlay of the bytes where its
   memory differs from the base; a split copies the overlay, never the memory. With one world the machine
   writes the base as before; when one world is left its overlay is folded in. Worlds compare memory by
   their overlays: normal overlays over one base are equal exactly when the memories are, and the ordered
   map compares and hashes in address order, so the order of writes does not matter. Measured: the
   bijection checks with memory owned and as an overlay over two bases (115,080 single steps, 24,576
   rewinds, 384 reading calls and 384 merging rewinds, representation included); 400 pairs of worlds
   written in random orders merge exactly when their memories are equal; the gate programs with each
   world also writing memory agree with the dense calculation and the whole-world reference (150 equal,
   32 refused where the calculation says); eight worlds writing `n` bytes each copy `4n` bytes at splits
   and hold at most `8n`, at 16 KiB, 1 MiB and 4 MiB alike (a full copy: 7 times the memory size). In a
   release build at 16 MiB of memory the peak resident size went from 134 MB to 19.7 MB and the time
   from about 90 ms to 7 ms; at 1 MiB, 11.5 MB to 4.4 MB. On the existing tests that split (1,234 runs,
   5,640 splits) splits copied 7.5 MB of memory before and 0 bytes now. What a split still copies is the
   journal, which grows with steps run (2.77 MB after 4,096 bytes written per world, against 16 KiB of
   overlay); sharing its common prefix is the route if it matters. `Stats` and `--stats` report the
   overlay bytes copied and held and the journal bytes copied.
   The independent testing pass of the overlay and of return addresses (`948dc92`, sol) found no wrong result: 512
   generated memory programs against the whole-world reference (writes restoring base bytes, overlapping and
   unaligned writes of every width, folds then new splits, merges after different write orders and un-runs),
   1,458 dense un-run cases (half with overlays), split statistics against independent counts; 63 device
   return-address programs, 480 C comparisons with native and 320 musl-linked results.
   **Fourth step: gates that take addresses** (`56b2c0b`; `docs/DESIGN.md`, "Qubits in memory";
   `crates/exec/src/cells.rs`). A gate's qubit is `%r[i]` or `[ADDR:BIT]`, registers and memory mixed; a
   keep may go into memory, `keep (v -> [ADDR])`. A byte that holds qubits is a cell: an 8-qubit register
   bound to its address in the world's register table, the memory under it 0, the base never quantum. The
   first gate at a byte promotes it (journalled); un-running the gate un-promotes, checked as the un-run of an
   allocation. Loads, stores and jump records over cells read them as a register is read (the world splits
   per value; un-running checks the cell still holds what the step read or wrote); a region's branch sees
   its configuration's values in its private memory. Keeps into memory need the bytes to hold qubits since
   before the mark. Registers, `qalloc`, `qfree`, register gates and `apply` are unchanged and the front is
   untouched. Measured: 8,616 single steps (552 faulting) and 12,288 rewinds are bijections by brute force,
   clean and dirty history, three memory modes; a splitting load and store and the merging rewind are the
   linear extension of bijections (192 starts each); `worlds_oracle.rs`'s 182 programs in all 8
   register/memory placements of q, r, o give the dense calculation's, the register run's and the
   whole-world reference's state (the reference run on the register form: it does not model memory
   qubits), 1,200 equal, 256 refused exactly where the calculation says; the worlds pass's F1 in memory
   faults; 14 mutations caught but one (cells in world equality, implied by equal journals). Cost against
   the register form (Grover in the history idiom, k = 1, 2, 8): the same worlds, splits, merges and
   configurations; steps +2(k - 1), world steps +32k - 2 (a load and a truncation read x), journal +30
   bytes whatever k, qubits held 16 against 5 (a cell is a whole byte). The checks of cell reads and
   un-promotions below a split point go through main's deferred checks (`d6036a1`), rebased on it.
   The independent testing pass of address gates and of the deferred checks (`f262f54`, sol; the whole-world
   reference extended to qubits in memory) found no wrong result: 1,456 dense placements, 384 generated mixed
   register and memory programs, 12,816 deferred-check cases, sums that cancel only over four worlds, 63 budget
   stops between a rewind and its checks reversed exactly. The 64-bit key: 1,048,576 distinct states, no
   collision; a birthday search would need about 5 x 10^9 states (about 81 GB), so a constructed collision is
   practical and the equality-on-match route stays a debt.
   **Fifth step: `apply` as mark, call, rewind-keep** (`78ea062`; `docs/DESIGN.md`, "`apply` as mark, call, rewind-keep";
   `crates/exec/src/region.rs`). The executor now computes every `apply` as the idiom on worlds, the region's
   rules on the call: mark the journal, call `@f` reading the argument registers as integers (one world per
   value), run each world to its end with effects, `exit` and quantum instructions faulting and the budget
   counted on each world's own steps (a world split off inside the region by a load of qubits in memory
   inherits its parent's count), keep the result into `out` or the fault code into `fault`, un-run to the mark
   checked as a rewind checks, merge. Strict region faults un-run with no keep and stop with the same
   `region fault`. It stays one step and one journal record of the calling run. Branch enumeration is kept
   as the comparison (`Executor::set_apply_mode`, `BENNETT_APPLY=history|branches|compare`); compare mode
   computes each `apply` both ways from the same state and panics at any difference in the result, the
   world's machine state and journal, its amplitudes (exactly) or the promised counts (`steps`,
   `region_steps`, region faults and weights, `rewound_steps`, `rerun_steps`, splits). The journal-content
   observer still enumerates (it classifies branches). The front is untouched; the text form unchanged.
   **Shown equal on every example and test that uses `apply`**, not a sample: with `BENNETT_APPLY=compare`,
   `cargo test` compares 107,687 `apply` executions in 46 tests (325 strict region-fault stops),
   `front/test.sh` 21,866 in 41 tests (13.9 million configurations; 76 strict stops), the ignored heavy runs
   110,121 more (the whole-world reference's rerun of the suite among them); no difference. Both suites
   pass with each way alone (`cargo test` 245, `front/test.sh` 155, in history, branches and compare mode).
   Regular tests keep it checked: `apply.rs` (every basis state of the exhaustive harness, faulting ones
   included, and the superposed arguments, strict and allowed, each reversed) and `apply_history.rs` (every
   entry of the three examples over 16 seeds, strict and allowed, reversed; the known exponential).
   **Cost** (`Stats::region_cost`, compare mode, summed; `docs/DESIGN.md` has the detail): over the front's
   tests the idiom makes 5.57 million runs where enumeration makes 13.9 million (a world per argument
   value), 0.80 times the work, 1.02 times the journal bytes, 1.46 times the time (45.1 s against 30.9 s);
   over `cargo test` 0.90 the runs, 0.81 the work, 1.20 the journal bytes, 2.6 times the time (small
   regions). The known exponential inside a region costs the same per run both ways (14,311 steps per run
   at depth 8 cleaning up, 183 with the history open); the idiom needs half the runs there. A region holds
   two copies of its amplitudes (0.89 GB against 0.43 GB on the 2^20-configuration in-place updates).
   **Whether the new instructions now pass every existing test** (the documents' condition for `apply`,
   `qalloc` and `qfree` to stop being needed as separate forms): for `apply`'s meaning, yes: every existing
   test and example passes with `apply` computed by mark, call and rewind-keep, faults and budgets included,
   and gives exactly the enumeration's state. As forms, no, and nothing is removed: the text idiom (`mark`,
   `call`, `rewind keep`) has no fault register and no region budget, so a program cannot write a region
   that faults on some branch or spends a budget without `apply` (question A1 below); `qalloc` and `qfree`
   were not re-expressed, and cannot be yet: `measure` takes a register only (question M2), an `apply`'s
   `out` and `fault` and a call's register argument are registers, and a qubit in memory costs a whole byte
   (8 qubits) in the simulator; Grover with its qubits in memory (`memory_qubits.rs`) is the one measured
   program. The maintainer decides.
   Its independent testing pass (`38e10c4`, sol; `crates/exec/tests/apply_independent.rs`, ignored witnesses) found:
   **F1** a `mark` value is visible as a result and depends on the caller's journal (history 4, enumeration 2),
   so two identical `apply`s no longer XOR the same result (`apply` is its own inverse no more); **F2** a strict
   region fault reports `unreachable` instead of `divide_by_zero` when the region's writes to memory qubits reorder
   the configurations (the first fault is to be in configuration order); **F3** compare mode misses 6 of 24
   mutations of the history path (`steps`, `merges`, `peak_worlds`, a changed budget, `in_region`, a leftover
   backup world); **F4** the whole-world reference numbers marks from 1, the executor from 2. F1-F3 fixed
   (`f16a55b`): a mark in a region counts records from the region's start, as on the region's own journal (the
   enumeration's numbering; `docs/DESIGN.md`, "History"); a strict fault's configurations are taken after the
   un-run; compare mode restores and compares every field of the run outside the world, the other worlds
   exactly (30 corruptions of the history path caught by `src/compare_tests.rs`, 24 of 24 by the scratch
   matrix). The witnesses are regular tests, with whole-class checks (5 region bodies under 5 caller prefixes,
   20 cell rewrites with analytic first faults); no cost difference measured beyond the host's noise. Compare
   mode: `cargo test` 110,198 `apply` executions (350 strict stops), `front/test.sh` 21,882 (76).
   F4 fixed in the reference (`36f7ae4`: public mark numbers modelled apart from its snapshots; 88 region
   comparisons added). Held: 65 budgets in
   all three modes on memory-qubit regions with full reversal, 131,072 configurations against the analytic XOR.
   Peak memory measured 260 against 200 MiB at 2^20 configurations on its own fixture (the published 0.89 against
   0.43 GB was not reproduced on that program's layout).
   The implementer's order is done: 1 mark/rewind, 2 worlds, 3 the memory overlay, 4 gates that take
   addresses, 5 `apply` as mark, call, rewind-keep.
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
   short formats is refused by name (it calls `powf` natively). `setjmp`/`longjmp` are done (`31f5138`); then the
   remaining intrinsics.
6. **A SQL query as an oracle.** SQLite runs, reverses and acts as a checked N=4 coherent oracle
   (experiment above), and Grover passes N=4/8. Next: grow the size/query range;
   the C front's memory-writing fallback remains necessary to remove the hand-written device entry.

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

- **The journalled machine (step 3), from its first step.** (1) Once worlds differ (its next step), keeping a
  result into a register the region read can make two worlds equal after the un-run, so their amplitudes add:
  not merely unspecified but not unitary. Proposed: detect it and stop with a named fault for now (the warning of
  step 4 needs the same detection), keeping later choices open. The same thing on today's machine (F1: a
  temporary not back to zero) is now detected and faults `bad_rewind`, changing nothing (`d012728`); that is the
  orchestrator's interim decision and the place a warning or another outcome would go. (2) The 4-bit fault codes are full (`bad_rewind`
  took 15); the next fault needs wider codes, and every `q4` fault register in the examples and the front with
  them. Widen, or another scheme? (3) A rewind across a measurement faults, as reversal does; "What is promised
  when history is left behind" says un-running after a change is a warning, not a refusal. Keep this one a fault?
  (4) A value defined between a mark and its rewind still reads its mark-time value afterwards; the validator
  allows such uses. Reject them?
- **Worlds (step 3's second step), with the reversible choice taken.** (W1) Is time part of the machine state? A
  world can rewind into a state another world reached and already left (only by rewinding a shared mark again):
  without a clock they interfere, on hardware padded to a budget they do not; the run stops with
  `RunError::Unaligned`. (W2) Question (1) above, refined: adding equal worlds' amplitudes is interference and
  exact; what breaks unitarity is un-running a read whose register changed; detected exactly, `bad_rewind`; rule
  4's warning is the route (keeping `v XOR q` as history, with channels). (W3) Should a fault in some worlds stop
  the whole run by default? (W4) Is observing which world at a measurement, and sampling one at the end, the
  intended reading of rule 1? (Counted in `Stats::observations` for step 4's warning.) (W5) Answered by the memory
  overlay: a split copies the bytes the world wrote, not the memory; it still copies the world's journal.
- **Qubits in memory (step 3's fourth step), with the reversible choice taken.** (M1) A keep into memory
  needs the bytes to hold qubits since before the mark, else `bad_rewind`, as a register kept into must be
  allocated before it: only then is every access since the mark journalled with its value, so un-running
  finds a keep that changed what the region read or wrote. Allowing a keep into a classical byte would need
  every load and store to journal its address too. Keep the rule, or pay for addresses in the journal?
  (M2) `measure` takes registers only; a qubit in memory is read by a load, which splits worlds, and
  observed where a measurement or the end of the run observes them. Should `measure` take an address (it
  would collapse the cell's configurations, as for a register)? (M3) A `longjmp` whose record is held in
  qubits that disagree faults `bad_indirect` instead of splitting into worlds that jump apart and may fault
  separately: a debt, unless the maintainer prefers the split.
- **`apply` as mark, call, rewind-keep (step 3's fifth step), with the reversible choice taken.** (A1) The text
  idiom has no fault register and no budget: a fault in the called function ends the run (with several worlds, that
  world, or a `WorldFault` stop), and its steps count against the run's budget. The executor's lowering of
  `apply` has both, so `apply` stays the form that carries them. Route if `apply` is to go as a form: a
  call form that gives the fault code as a value and takes a budget (for example `%y, %c = call @f(args)
  budget B`, then `rewind %h keep (%y -> out, %c -> fault)`), with the region's rules (no effect, `exit` or
  gate) attached to it. Add it, or keep `apply`? (A2) Inside a region the worlds run one after another and
  merge only at its end, never in lockstep ticks: merging earlier would join configurations that have
  spent different amounts of the budget, so the budget would make time part of the state (question W1).
  Keep that as the region's rule? (A3) A mark's handle is an ordinary `i64` that code may compute with or
  return; the documents do not say whether it may be observed. Taken, as the reversible choice (`1c8e8c3`): in
  a region it counts records from the region's start, so a region's result never depends on the caller's
  history and `apply` stays its own inverse; it still depends on the region's own path (how many records
  it wrote). Keep handles observable, or make them opaque (the validator refusing any use but `rewind`)?
- **Return addresses** (`2bf0b69`): the outermost frame gives `0xFFFF_FFFF_0000_0000` (natively an address in the C
  library's startup) and a level beyond it 0 (natively unspecified). Keep? Levels above 0 follow the device's
  frames, which natively needs frame pointers kept and sibling calls off (plain native `-O2` crashed at level 1).
- **Calls through another prototype** (`329ef7a`; its independent testing pass `4409160`, sol: 2,880 native
  comparisons, 72 coherent branches, 1,101 further runs and 27 corpus programs, no wrong result): every call through a function pointer now compares its target with
  the address-taken functions the device would call wrongly (musl tests: 107 to 353 more steps, 0.06 % to 5 %, and
  5 % to 16 % more device code). Cheaper: compare only for unprototyped pointer calls (about 11 steps), leaving a
  typed pointer call to a function with other argument or result sizes a `bad_indirect` fault instead of agreeing
  with native. On the musl corpora (`64571ac`) every program that agrees takes more steps (median 2.0 % GCC, 1.5 %
  c-testsuite, 1.8 % `ieee/`; largest `bcp-1` +21.8 %), the dispatch in musl's own calls through pointers. Keep
  full coverage?
- **`setjmp`/`longjmp`.** (1) `docs/DESIGN.md` ("Non-local jumps") lets a stale jump record succeed when a later
  call of the same function sits at the same depth and stack address (C leaves it undefined; the testing pass's
  probe returned 77). Keep that, or fault by name? (2) Since `595339c` a copied `sigjmp_buf` carries its own saved
  mask and the mask is restored before the jump, as glibc (the native reference) does; musl's assembly restores
  from the original buffer, which differs only if the original is saved again after the copy. Keep glibc's?

- **Complex division: compiler-rt or libgcc?** compiler-rt's division (decided) fails GCC's three `cdivchk`
  accuracy checks natively, which pass with libgcc, the runtime of the corpus's native reference. When the
  `FENV_ACCESS` refusal goes they will be counted as wrong results. Keep compiler-rt and compare those against
  a `--rtlib=compiler-rt` native build, or vendor libgcc's division (GPL with the GCC runtime exception), or
  something else? (`rt/compiler-rt/README.md`, "Not libgcc".)

- **Floating-point environment**: keep the named fault for non-default rounding modes and flags, or make them real
  through SoftFloat's state? (The float runtime lets the optimiser delete its flags today.)
- **Exit status**: the device reports the full 32-bit `exit` value; a process status is its low 8 bits. The corpus
  harness compares modulo 256, the integration tests exactly. Mask at the boundary?
- **The fault code space is full**: `bad_rewind` took code 15, the last that fits `Fault::CODE_WIDTH` (4 bits;
  a fault register is at least that wide). An invalid jump record therefore shares `bad_indirect` (a control
  transfer through a value that names no valid target) instead of a `bad_longjmp` of its own. Widen the code
  to 5 bits (every fault register one qubit wider; about 24 hand-written programs allocate 4) and give it its
  own code, now or when the next fault comes?
- **Time and randomness on the device** (clock start value, entropy image): needed before the earlier workspace's
  clock and entropy models can be carried over (`docs/SALVAGE.md`).
- **Equal-priority constructor order across separately compiled objects** follows a native build of the linked
  module, not separately linked objects (C leaves it unspecified). Record as a debt, with the route of lowering
  startup per unit before linking?

## What is still refused, and what removes it

| Refused today | What removes it |
|---|---|
| Files, clocks and the other unimplemented system calls (named faults) | Step 1 above |
| A call through another prototype where x86-64 does not determine what is read: the upper half of a 32-bit value, a missing argument that is read, an integer from a floating-point register, a variadic floating-point argument without `%al`, a narrow argument that is not the extension the callee relies on (named fault `unreachable` when the call runs); a direct call whose argument differs from its parameter only in register class (native -O0 and -O2 disagree) | Nothing to remove: the native result is not determined (`docs/DESIGN.md`, "Calls through a prototype"). Conservative, removable by a finer model where named: a variadic floating-point argument after an unread named one, a call whose result is used only by dead code before the optimiser, values in memory read at another place |
| A qubit-dependent call of a function that uses `setjmp` | The front's own effect summary of `setjmp` (writes only its buffer) when deciding what runs coherently (`docs/DESIGN.md`, "Non-local jumps") |
| `__float128`; `powi` on `_Float16`/`__bf16`; `#pragma STDC FENV_ACCESS` (with it musl's `ilogb`, and so `__divxc3`, the `long double` complex division); non-default rounding modes and floating-point flags | Step 5 above |
| A store under qubit control; an update with no known inverse; a value needed after it changed; qubit variables in structs, heap objects or passed to functions; gates in a loop under qubit control | The journalled machine (step 3; the device holds qubits at any address since `56b2c0b`, the front's fallback, step 4, is what uses it) |
| A qubit-dependent value used by ordinary code or printed; measurement under control; a dirty discard | Channels |
| Non-empty inline assembly; code without source | An emulator of the real machine, compiled as an ordinary program |

## Known weak spots

- Two equal XOR keeps into a memory cell read since the mark cancel, but the early read-target check
  still refuses them with `bad_rewind` (`canceled_memory_keeps_preserve_a_read_and_its_interference`,
  ignored in `crates/exec/tests/memory_qubits.rs`). The check considers each keep rather than their net XOR.

- Worlds' deferred un-run checks (`d6036a1`) are matched across worlds by a 64-bit hash of the classical state; a
  collision would add two different checks (route: compare the states on a hash match). The copy each world saves
  before a rewind makes the 1,500-iteration idiom loop about a third slower (route: copy only the journal above
  the mark). When a sum fails, every waiting world of the group that rewound last faults; the reference model
  faults every world at that rewind (no generated program differs).

- The `long double` increase is bisected to `8bbb56c`: packed `i80` results and changed round-pack inlining
  add 189/209/20,671 steps in `960215-1`/`regstack-1`/`00204` (normal add/sub probes +9/+10); this buys
  memory-free aggregate returns under qubit control (`rt/softfloat/README.md`).
- c-testsuite `00040` passes at exactly 489,693,622 steps with native output; 97.28% is its repeated `chk`
  board scan, versus 391,839,420 native `-O2` user instructions (1.2497x; `experiments/c-corpus/README.md`).
- A small aggregate result that contains a pointer is still returned through memory, so it is refused under qubit
  control at `-O2` (route: pack it too, after measuring what turning a pointer into an integer costs LLVM's alias
  analysis). A packed struct of 129 bits of bitfields becomes an `i136` and is refused (route: split memory
  operations LLVM widens past 128 bits before emission).

- Pointer-call checks: the worst five GCC increases are +462/+131/+83/+76/+67 steps, of which
  146/43/13/10/6 are compares; the rest is branch/select scaffolding, including folded constant-alias checks.
  `experiments/c-corpus/README.md` counts candidates per call and models unprototyped-only checks and uniform
  wrappers; neither cheaper policy is selected (typed mismatches lose coverage; wrappers write memory).

- The region budget counts device instructions, not the source-fixed unit the North Star asks for.
- A `_Bool` qubit costs 8 qubits.
- A qubit in memory is held as a whole byte, 8 qubits in the simulator (`docs/DESIGN.md`, "Qubits in memory";
  route: cells of single bits, if the count matters).
- Some acceptance decisions in the front rest on one LLVM pass proving an identity; when it cannot, the front
  refuses, which is safe but brittle.
- Variadic calls follow the x86-64 System V convention and clang's inline `va_arg`, for every alignment up to
  LLVM's limit of 2^14 bytes. Above it clang 18 itself writes invalid IR (a `byval` with `align 1048576`; the
  native build crashes) and `bennett-front` aborts in LLVM's bitcode reader instead of naming the cause (route:
  a fatal-error handler around reading the input that reports invalid input by name).
- A struct passed by value from a call clang marked `tail` lost the stores that fill its copy once `byval` was
  dropped (a silent wrong result at `-O2` when the callee is not inlined; fixed with the over-aligned variadic
  work, `front/tests/c/byval_tail_call.c`). The front's other lowerings that pass a copy by address (the variadic
  area, values in memory) already cleared the mark.
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
