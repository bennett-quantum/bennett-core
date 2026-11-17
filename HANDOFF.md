# HANDOFF

State on 2026-11-17: **resumed by the maintainer the same day; see "Resumed on 2026-11-17" below before
anything else.** The tree implements the model stated in `docs/DESIGN.md`, "The model": ordinary code; the
quantum instructions it issues (`qalloc`, gates, `measure`, `qfree` as the partial trace), naming qubits in
registers or at memory addresses, with the named fault `qubit_memory_access` when ordinary code touches a byte
that holds qubits; and `apply`, evaluated for each definite value of its argument registers with one step bound
`B` for the whole applied function. `mark`, `rewind`, machine states in superposition and the call with a fault
value are removed. Steps 1 to 4 of the order below are done; the documents say what the code does now. Next:
the rest of step 5 (the reference that runs `apply` by its construction) and step 6, applied calls, which is
being built on its own branch.

## Resumed on 2026-11-17

Work was stopped on the morning of 2026-11-17 because the maintainer wanted to understand the computational
model before deciding anything, and was concerned that it might be unphysical. A rigorous account of the model,
with proofs, was written that day. What it showed:

- Ordinary code that issues qubit allocation, gates with controls and measurement is the standard quantum
  circuit model, for a bounded number of steps and finite memory.
- `apply @f(args) -> out, fault budget B`, defined for each definite value of the argument registers as running
  `@f` (ordinary code, no quantum instruction) for exactly `B` steps on a private copy of memory and XORing its
  result or fault code into a target it does not read, can be carried out by a device: each ordinary step is
  made reversible by keeping the previous state, the result is copied out by XOR, the inverse steps run, and the
  working memory returns exactly. One `apply` costs two runs of `B` steps.
- `mark`/`rewind` is proved only in a narrow static form (each mark paired with its rewind in the program text,
  nothing allocated, released, measured or printed between). The form the executor runs, where run-time paths
  choose what a rewind undoes, is proved only under conditions and open in general, and several of the
  executor's rules for it decide by inspecting amplitudes, by comparing parts of a superposition with each other
  or by one counter for the whole run, which no device can do.
- A release that faults when its register is not zero is not something a device can decide reliably: a nonzero
  part may have arbitrarily small amplitude.

No lowering of a device program to a circuit exists in the tree yet: the executor is a simulator of the
model's rules, not a compiler to gates.

The maintainer's decisions of 2026-11-17:

- **`qfree` is partial trace**: the register is dropped unexamined; a released register that is not zero is not
  a fault of the program. ("qfree can only mean partial trace. I don't see how measuring is stable here: what if
  nonzero has tiny prob?")
- **`mark` and `rewind` are removed** from the language, the validator and the executor. The last commit that
  contains them is `f81a70a`; they can be recovered from it. The proved static form can come back later with its
  test attached.
- **Gates on memory bytes keep their plain form**: a gate or a measurement may name its qubit by an ordinary
  address. ("ok keep plain form")
- **Ordinary code that reads or writes a byte holding qubits is a named fault.** ("yes, it should be a fault to
  read qubits")

`apply`, `qalloc` and `qfree` stay ("Decision the maintainer took on 2026-11-16, in the evening"); `apply` is
the only bridge from qubits to ordinary code.

The maintainer's further points, about 13:00, 13:15 and 13:40 UTC:

- **Memory written under `apply` (about 13:40 UTC).** The rule put to the maintainer: "memory writes made by code
  run under `apply` do not survive it; only the returned value does." The answer: "ok I accept rule 1 in the
  following sense: sqlite. I want to be able to apply sqlite. I understood that to be entirely possible by
  ensuring all memory allocs are *also* simulated and applied". The runtime already has the property: musl's
  allocator, the file model, the clock and the entropy image are ordinary device code over device memory, so under
  `apply` they run on its private memory and are undone with it.

- **Goals, not started:** "a lean proof of the executor is a medium term goal"; and, on the requirement that each
  instruction's step is a total function of the machine state, "this is a followup lemma. partial functions are
  probably fine in the standard way." Meanwhile the executor's definition of `apply` and of each ordinary step
  is kept small and plain enough to be stated formally later.
- **Derived forms:** "If the front emits more, that is also probably fine, as long as the map to 1-9 + apply is
  obvious, or straightforward." A form beyond ordinary code, `qalloc`, gates, `measure`, `qfree` and `apply` is
  acceptable when it is defined by a straightforward expansion into them, and the documents define it by that
  expansion.
- **Arbitrary angles** are part of the gate set ("there really is no harm in arbitrary angles"); a device with
  a finite gate set approximates them.
- **Step bounds:** a fixed step bound outside `apply` is not a limitation to state ("same holds for normal
  computation"). Inside `apply` there is exactly one bound, `budget B`, on all the steps of the applied
  function, its loops, nested loops and calls included; never a bound per loop ("I 100% definitely require the
  capability to run unbounded classical loops depending on input under apply."). The bound may be a value
  computed at run time.

The order of the work:

1. Done: the executor computes `apply` by evaluation for each definite value of the arguments only; the mode
   switch, the comparison mode and the computation through marks are gone.
2. Done: `mark`/`rewind` left the reader, writer, validator, executor, CLI, examples, fixtures and tests, with
   what existed only for them: a call passing a qubit register to ordinary code outside `apply`; the call form
   returning a fault code; deadlines and clocks on marks; leftovers; the scheduling and joining of machine states
   in superposition; also the memory overlay, which existed only so that such machine states could share one
   base. Stepping an ordinary run backwards stays where it is exact (ordinary steps, gates, `apply`) and refuses at
   a measurement.
3. Done: `qfree` is the partial trace (`docs/DESIGN.md`, "Releasing a register"): no release faults; the
   simulator keeps a register released while not zero as an unnamed column, so the stored state stays the exact
   purification, and reports the count and weight of such releases, which no program can read. The fault
   `dirty_free` (code 11) is removed and its code stays unused. The front's tests read the report instead of the
   fault (their `run` fails a test on a nonzero release). Ordinary code that reads or writes a byte holding
   qubits (a `load`, a `store`, a jump record) faults `qubit_memory_access` (code 17), in every configuration,
   whatever the qubits hold, inside an applied function too (per argument value); the simulator's stop
   `SuperposedMemoryRead` and the reading of qubits in memory by applied functions are gone (`docs/DESIGN.md`,
   "Qubits in memory"). A program applies a function to qubits in memory by copying them into a register with
   controlled X gates first. The simulator's own stops and reports (amplitudes of rounding size dropped, the
   configuration limit, the stop at an `apply` faulting for some argument values, amplitudes in snapshots, the
   report of nonzero releases) are listed in one place in `docs/DESIGN.md` with why none gives a program a value:
   no instruction reads them, a stop ends the run, and inside an applied function every quantum step faults
   before a stop could arise (a test checks the last with the configuration limit set where it would bite).
4. Done: the documents say what the code does now; `docs/DESIGN.md` states the model, `apply`'s meaning, why a
   device can carry it out and what it costs, and defines the derived forms (`call_indirect`, `stacksave`,
   `stackrestore`, `retaddr`, `setjmp`, `longjmp`, `exit`) by their expansion into ordinary code.
5. Independent checks: an audit of the front against the model (done: three wrong translations fixed, "Known
   weak spots"; two findings on loops go to step 6), a testing pass over steps 1 and 2 (done: 13 tests, no
   defect; "Independent checks of apply, removal and front fixes"), a testing pass over step 3 (running), and a
   small reference that runs `apply` literally by the device's construction (not started).
6. Then the route that replaces the front's cancelled fallback ("Next steps, in order", 4): a call whose
   arguments depend on a qubit, to an ordinary function (no quantum operation, directly or through anything it
   calls), is applied whole with one `budget`, whatever memory it writes, allocates or frees and whatever loops
   and calls it contains; its writes are undone when it ends and only its result survives (a console write or
   `exit` inside is the named fault). Acceptance target: `apply` of a function running a SQLite query whose key
   depends on a small qubit register, on a database set up by ordinary code before the `apply`, each argument
   value compared with native SQLite. Writes that must survive the applied code are planned separately.

Debts opened by step 2 (removing `mark`, `rewind`, machine states in superposition and the call form):

- **A second implementation of the device language.** The independent reference interpreter modelled the
  removed machinery and went with it; the C rungs 1 and 2 (`experiments/northstar-ladder/`), the musl entry-frame
  edges and several executor tests lost it as a second executor and keep their native or hand-worked
  expectations. Route: an independent reference of the current language, starting from the reference that runs
  `apply` by its construction (step 5).
- **The fallback oracle's self-tests on a fallback-shaped `store`** (`front/tests/tests/fallback_oracle.rs`, module
  `seeded`) built the program with a call reading a register, which ordinary code may no longer do; they were
  removed, and come back with the route that replaces the fallback (step 6), on a program the model expresses.
  The self-tests on coherent programs and on applied regions stay.

Not to be landed: `wip/fallback-step3`, `wip/sol-mutants-machine` and `wip/sol-mutants-machine-partial`. They
build on what is being removed and are left where they are.

Not started: idiomatic modular quantum C with Shor's period finding as acceptance, Grover over a SQL query from
plain C, the floating-point environment.

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
| `cd2d920` | The complex multiply and divide helpers: compiler-rt 18.1.8 vendored unmodified (since replaced by libgcc; `rt/libgcc`), linked by the front where a program calls them; bit for bit with the same helpers on the hardware, NaN bits aside |
| `31f5138` | `setjmp`/`longjmp` (library and built-in forms, `sigsetjmp` with musl): a device `setjmp` instruction and `longjmp` terminator, journalled (`docs/DESIGN.md`, "Non-local jumps") |
| `2bf0b69` | Return addresses: `%r = retaddr N` reads the device's frames (`docs/DESIGN.md`, "Return addresses"); GCC `20030323-1`, `20030811-1`, `pr17377` agree with native, with musl and without |
| `595339c` | `sigsetjmp`'s signal mask kept in the jump environment: the two wrong results of its testing pass fixed, checked against native on every small way to save, copy and jump |
| Lua probe | Unmodified Lua 5.4.9 through musl: seven native stdout/status comparisons and exact reversals; a Lua predicate under hand-written `apply`, dense-checked Grover at N=4/8 (`experiments/lua-probe`) |
| `c5085b4`..`ee41593` | The model of 2026-11-17: `apply` for each definite value of its arguments only; `mark`, `rewind` and what served them removed; `qfree` the partial trace with the simulator's report; the fault `qubit_memory_access`; the simulator's own stops named; three wrong translations of the front fixed |

Check it: `cargo test` (253 passed; 3 ignored, each naming its reason: heavy reference, generated or
hash-sample runs with their commands, witnesses of recorded defects and of questions awaiting the maintainer;
about 9 s, no LLVM needed) and `front/test.sh` (222 passed; 26 ignored by default, each naming its reason: the
fallback acceptance tests, waiting for the route that replaces the cancelled fallback, the C North Star ladder runs and other explicit experiments,
musl-linked checks that need `rt/musl/build.py` first, heavy runs, and witnesses of recorded debts; needs LLVM 18, clang 18, CMake). `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are clean.

With musl linked, standard output and exit status compared with native (`experiments/c-corpus/README.md`):
GCC's C torture tests **1,574 agree, 1 refused by name**, 81 skipped;
c-testsuite **219 agree, 1 over the step budget**;
GCC's `ieee/` **64 agree, 3 refused**, 1 skipped; **none wrong or crashing** (`1e06f2f`, 2026-11-16, night).
All 1,944 programs compared by name with `64571ac`: exactly six `unreachable` faults become agreement
(GCC `fprintf-2`, `printf-2`, `user-printf`, `loop-2f`, `loop-2g`; c-testsuite `00187`); every other outcome
and normalized reason is unchanged. The file/clock runtime supplies the previously missing operations. The
old fault frames identify `clock_gettime` (228) in the three temporary-name programs and `open` (2) in
the other three. All 3,716 cross-executor replays (both directions for the 1,858 numeric programs) reproduce
outcome, status, console and exact steps; the executor changes cause none of these classical deltas.
The two loop cases open `/dev/zero` then return on this 64-bit host; their mapping/loop paths are not measured.
Three workers maximum, sequential runs under the shared heavy slot, musl linked, 1 MiB stack,
10^8-step budget, `ieee/` with `-fno-inline`; wall times GCC 1,715.70 s, c-testsuite 252.67 s, `ieee/` 74.57 s
(queue excluded; shared machine, not a throughput comparison). Unchanged-outcome numeric pairs:
1,850 up, one down (`complex-5`, 3,305 → 3,258), one unchanged (8-queens at the budget); medians extra steps /
relative increase: GCC 8 / 0.3800%, c-testsuite 21 / 1.0106%, `ieee/` 8 / 0.3616%.
Report and every moved case: `experiments/c-corpus/README.md`; ignored frozen binaries, complete native/device
artifacts and comparisons: `.tmp/corpus-night/results-1e06f2f/` in the testing checkout. Previous prototype-call
cost attribution and the `eeff78d` comparisons remain in the README's `64571ac` section. Without the library
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
- **What is promised about coherence** (`docs/DESIGN.md`): classical results are exact; coherence is
  guaranteed for a documented class of updates and for anything un-run; elsewhere it is unspecified and the
  toolchain warns; un-running after something changed what the region read or wrote is a warning, not a refusal.
- **The next extension**: a journalled machine with `mark` and `rewind`. Built from 2026-11-15, removed on
  2026-11-17 ("Resumed on 2026-11-17"); gates that take addresses stay.
- **Channels** (partial trace, measurement by effects, noise) belong to the finished Bennett, and nothing built
  now may make channels impossible to add. Since 2026-11-17 `qfree` is the partial trace.
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

## Next steps, in order

1. **The musl runtime is done** ("Stopped in flight"). Files, clocks and entropy now use the carried models
   (`rt/musl/README.md`, `docs/DESIGN.md`, "Files, clock and entropy at the musl boundary"). The remaining
   runtime debts and routes are listed below. `setjmp`/`longjmp` are done (`31f5138`).
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
   A global-write extension measured log/hash/B-tree insertion with the machinery removed on 2026-11-17 and
   `apply` drivers; its record is in `experiments/journal-content/README.md` and `global.tsv`.
3. **Independent checks** (step 5 of "Resumed on 2026-11-17"): the reference that runs `apply` by the
   device's construction (each ordinary step keeping what it overwrote, copy by XOR, the inverse steps in
   reverse order) on small functions with explicit amplitudes, checking that the working state returns to its
   start and that the result equals evaluation for each argument value, a faulting argument value and step
   exhaustion included. It becomes the start of a second implementation of the device language (the debt
   above).
4. **Applied calls, replacing the front's cancelled fallback through marks** (the maintainer's decision of about
   13:40 UTC, "Resumed on 2026-11-17"; being built): a call whose arguments depend on a qubit, to an ordinary
   function, is applied whole with one `budget`, whatever memory it writes, allocates or frees and whatever
   loops and calls it contains; its writes are undone when it ends and only its result survives. Today the
   front applies a function only if LLVM proved it reads memory only, and refuses a qubit-dependent call to one
   that writes memory; the route lifts that. Acceptance target: SQLite under `apply`, a query whose key depends
   on a small qubit register, each argument value compared with native SQLite. Planned separately: writes that
   must survive the applied code (an inline store under a qubit-dependent condition that later code reads), with
   the two loop findings of the front audit ("Known weak spots"). The target for those is the fallback oracle
   (`front/tests/tests/fallback_oracle.rs`; fixtures and explanation in `front/tests/c/fallback_oracle/`): host
   clang runs each C update once per basis input, the device result is compared with it, and its ignored
   acceptance tests wait for the route. The Boolean XOR on a quantum `_Bool` (`y ^= q && !r`, and
   `if (q) if (r) y ^= 1` at O0 and with `--taint-all`) already passes it (`coherent_xor`, `coherent_nested`).

5. **Floating point** is done over Berkeley SoftFloat 3e (decided 2026-11-15; `rt/softfloat/README.md`), the
   complex helpers over libgcc (`rt/libgcc/README.md`), including `__divxc3`. Left: `__float128` and
   constrained operations (`FENV_ACCESS`, including the corpus checks' own musl `ilogb`/`ilogbl`).
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
   (experiment above), and Grover passes N=4/8. The opt-in larger experiment now checks N=16/32/64/128/256
   for LIKE, integer equality, TEXT ordering, EXISTS with two WHERE conditions and a range aggregate:
   native forward, full byte-exact classical reversal, coherent output 0/1 against the independent
   dense matrices (and full reversal), and Grover with every amplitude checked. Per-mode steps,
   journal, wall time and peak RSS are in `experiments/sqlite-on-device/measurements.tsv`; commands
   and compact tables in its README. The original 256 KiB SQLite heap refuses N=128 at LIKE row24
   with `SQLITE_NOMEM` identically native/device (7,893,757 steps); N=128/256 pass with 1 MiB heap.
   The comparison of `apply` through marks with evaluation for each argument value (both since removed)
   also passed N=16/32/64. All five Grover shapes pass N=512;
   the full coherent checks there hit the declared 300 s process cap (LIKE during full
   reversal; other shapes during their second apply); aggregate classical reversal hits the 4 GiB address-space
   limit while building the undo journal (2,322,360 KiB peak RSS). These are shared-host experiment bounds,
   not physical qubit/RAM limits. The baseline sweep predates the atomic-rewind fixes; validation
   on the rebased executor passes all 57 checks: full N=16/32, every N=512 Grover shape, and both
   N=256 aggregate coherent reversals, with unchanged step/journal counters (separate table).
   The fast glue test exposed an entry-stack refusal panic; its minimal ignored witness is
   `sql_oracle_glue::an_entry_frame_that_does_not_fit_returns_stack_overflow`. The executor is not
   changed by this experiment. Next: larger process budgets remain unmeasured;
   the C front's memory-writing fallback remains necessary to remove the hand-written device entry.

## What the earlier workspace offers

- **The salvage survey** (`docs/SALVAGE.md`, ordered by the next step that would
  use each piece, with what was re-run and what was only read). The one direct code carry is for step 1:
  `bennett-runtimes`' clock and entropy models, file system and system-call dispatch now run in `rt/musl`
  (provenance: `CARRIED.md`), following the 2026-11-16 clock/tape input decision. Remaining runtime debts
  and their routes are below. For step 5,
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
  which called `logbl` and `scalbnl` (and through musl's `ilogbl` a `FENV_ACCESS` pragma).
  **Superseded on 2026-11-16:** libgcc replaces all six; its `__divxc3` has no library dependency.

## Decisions the maintainer took on 2026-11-16

Every question in "Questions the maintainer answered on 2026-11-16 (kept for their detail)" is answered. The
labels in brackets are that section's. Four of these were changed or refined the same afternoon ([(1), W2], [W3],
[W1], [A1]): see the next section.

The machine (its decisions on `mark`, `rewind`, machine states in superposition and the call form went with
them on 2026-11-17; git has them):

- **Time and randomness on the device**: the clock is a start value given to the run plus the number of clock
  reads so far; entropy is a seeded byte image given to the run. Both are inputs of the run and journalled like
  any other state. This unblocks files and clocks (step 1).
- **`measure` takes an address** [M2]. Built: `%d = measure iN [ADDR]` reads the `N` bits from bit 0 of the
  integer at the address as a load does; the bits of bytes that hold qubits, only those, are sampled jointly
  and collapse; other bytes give the memory's bits (`docs/DESIGN.md`, "Qubits in memory";
  `crates/exec/tests/measure_memory.rs`, against the independent reference of the time). The register form stays while
  registers do.
- **Fault codes widen to 5 bits** [(2), "The fault code space is full"], now; an invalid jump record gets its own
  code. Built: `Fault::CODE_WIDTH` is 5, every fault register a `q5` (the front's included), and every
  `longjmp` that finds no valid record, also one held in qubits that disagree [M3], is `bad_longjmp` (16);
  `bad_indirect` (4) is a call target again.
- **A `longjmp` whose record is held in qubits that disagree** [M3] stays a fault; a debt.

C:

- **Complex division follows libgcc**: all six complex helpers come from libgcc (GPL with the GCC runtime library
  exception), replacing compiler-rt's, so that the device equals the corpus's native reference. This supersedes
  the 2026-11-15 choice of compiler-rt. Built from verified GCC 13.3.0 (`rt/libgcc`); all six,
  including `__divxc3`, are self-contained. See the measured state above.
- **Calls through function pointers keep the full check.**
- **A stale `setjmp` record** keeps today's behaviour, as a known weak spot.
- **A copied `sigjmp_buf` follows glibc.**
- **Return-address values** at and beyond the outermost frame: kept.
- **The floating-point environment** stays a named fault until the front's fallback is built; then rounding modes
  and flags become real through SoftFloat's state. (The fallback through marks was cancelled on 2026-11-17; the
  route through SoftFloat's state stands.)
- **The exit status is not masked on the device.**
- **Equal-priority constructor order across separately compiled objects** is a debt; route: lowering startup per
  unit before linking.
- **`front/include/bennett.h` is made includable from C++.**

How work is done:

- **Ten automatic checks are built**: mutation testing of the changed lines before a step lands; a test that
  clang and LLVM are the pinned versions; a table of well-formed and malformed device programs the validator
  must accept or reject; automatic discovery of the example programs; a classification of every LLVM attribute
  kind in the front; a pre-push check of the exact revision being pushed; a guard against host paths, logs and
  stray files in the tree; no work-in-progress commits on `main`; a lint against catch-all match arms in library
  code; a flag on bare time constants in scripts.
- **Test-writing rules** drawn from past findings go into `docs/DESIGN.md`, "Tests".
- **Order of the next work**: fix the two replay panics and the canceled-keeps refusal ("Known weak spots";
  done on 2026-11-16: a failed rewind is atomic on every path, compared on the whole machine in
  `crates/exec/src/atomic_rewind_tests.rs`, and keeps are refused by their net XOR);
  the decisions above that change the device program and the executor; the front's fallback (step 4); files and
  clocks; the checks; then growing the SQL oracle (a cost at a large size needs a cost model, a design question
  of its own).

## Decisions the maintainer took on 2026-11-16, in the afternoon

Taken after the design audition (its own section below) and on the three questions then open ("Questions the
maintainer answered on 2026-11-16, in the afternoon"). Where one changes a decision of the morning it says so,
and it is the later one that holds.

- **NaN payloads.** A result that differs from the native one only in which input NaN's payload it carries
  counts as agreement, compared by a rule that allows exactly this. The bit-for-bit debt is dropped.
- **The coherent class stays as documented for now.** A controlled in-place update of ordinary memory
  (`cells[0] += 2` under a qubit) is not added to the guaranteed class. Adding it waits for an oracle that
  compares phases as well as weights, because the class may never shrink. (The decision also placed such
  updates under marks, which were removed on 2026-11-17; the front refuses them.)

Its decisions on rewinds, machine states in superposition, time and the call form went with them on
2026-11-17. The stop of the run at an `apply` faulting for some argument values stays the simulator's default.
## Decision the maintainer took on 2026-11-16, in the evening

- **`apply`, `qalloc` and `qfree` are not retired** until the maintainer decides otherwise: the forms may suit
  front ends for other LLVM languages. They stay first-class forms of the device program, with their tests,
  examples and documentation, and the front may keep emitting them.

## Automatic checks (implementation)

The ten checks and test-writing rules are implemented in `docs/CHECKS.md`, `docs/DESIGN.md` ("Tests"),
`scripts/`, `.githooks/`, the IR tests and the front checks. `bennett.h` compiles as C++ with bool pointers.
The hook is opt-in (`git config --local core.hooksPath .githooks`), checks every pushed nondeleted SHA in
an isolated archive, and rejects incoming WIP ancestors on main. It runs all four repository checks.
The pin is LLVM/clang major 18, as before; patch releases are compatible. Attribute classification is
exhaustive for LLVM enum kinds and records conditional ABI handling. The clang 18.1.3 input envelope
now has an inventory at both optimisation levels (7,597 emitted modules, 43 enum kinds and 16 string keys),
including enum/string values and every observed
function/return/parameter placement on definitions, declarations and calls (`docs/CHECKS.md`,
"Measured attribute placements and native comparisons"; complete counts in `docs/attribute-inventory.tsv`).
String policies were reviewed against native comparisons within that envelope; this is not an exhaustive
claim about arbitrary strings or calling conventions.

Debts and their routes: (1) automated changed-line mutation generation is Rust-only (cargo-mutants);
C++/script changes currently use recorded seeded edits. Route: integrate an established C++/script
mutation engine with diff selection and the existing suites, then demonstrate surviving-defect rejection.
(2) Whole-step mutation and isolated cold pre-push builds do not have a seconds-long total-time guarantee;
work scales with diff size and compilation. Route: measure/cache exact dependency artifacts and select
proven relevant tests, retaining complete changed-line mutation scope; no timeout or percentage gate.
The default tests and small guard witnesses stay fast. (3) Remaining attribute debt: non-IEEE
`denormal-fp-math` and `denormal-fp-math-f32` were accepted and silently kept IEEE subnormals in SoftFloat;
native MXCSR DAZ/FTZ satisfies the promised preserve-sign environment and disagreed at both O0/O2 (two
witnesses in `front/tests/tests/attributes_independent.rs`, run and seen failing). Since
`wip/small-fixes`, an operation that reads the mode in such a function is refused by name
(`front/src/Denormal.cpp`; "What is still refused"); the witnesses expect the refusal. Route: the
floating-point environment through SoftFloat's state.
The observed ABI/reference/allocation/no-builtin/ordinary-FP comparisons otherwise agree (704 exact result
words and 32 allocation hashes at both levels combined); strict FP arithmetic is refused by name. Still unproved: attributes/values/placements outside the measured input
envelope, generated sources only constructed by ignored experiments, nondefault calling conventions
(`inreg`, `preallocated`, `inalloca` have zero measured occurrences), native AVX512 execution on this CPU,
and the floating-point environment (route: SoftFloat's state). Expand the inventory/native matrix
when these inputs are added. No second exhaustive enum-name allowlist is maintained.

The attribute-evidence step's required checks after rebasing on `d3880f6`: Rust 499 passed / 29 ignored,
front 210 passed / 36 ignored; root clippy, the workspace-library lint, formatting and tree guards pass.
The new test target also passes clippy separately. An extra front-wide all-targets clippy run exposes two
pre-existing `unwrap()` calls in `front/tests/tests/complex.rs` (archive/commutation paths), left unchanged;
that broader harness lint is not covered by the root's default-package all-targets command.

The changed-line mutation run on the job's own diff (`python3 scripts/mutate-diff.py 2142238` at `0e1b00d`, 2026-11-16):
22 mutants, 16 caught, 5 unviable, 0 timeouts, 1 missed: `crates/exec/src/measure.rs`, `live_sources`,
`Undo::Longjmp => shadow.len() - discarded` with `-` replaced by `/`. The line predates the job; it is in the
diff because the job named every alternative of that match. No test drove a `longjmp` undo through the
live-source scan. Since caught: `a_longjmp_result_witness_names_the_frames_left_after_the_jump`
(`crates/exec/tests/journal_content.rs`) fails on the mutant twice over, by a division by zero for a jump in
the frame and by naming call path 2 instead of 0 for a jump from a callee, and passes on the code. The
first attempt's unmutated baseline failed because the libgcc scripts read `target/front` directly instead of
following `BENNETT_FRONT` (fixed in `0e1b00d`).

The mutation runs over the machine's changes of 2026-11-16 and 17 (machine step e, decisions a to d) concerned
code removed on 2026-11-17; git has their records.

## Questions the maintainer answered on 2026-11-16 (kept for their detail)

- **Return addresses** (`2bf0b69`): the outermost frame gives `0xFFFF_FFFF_0000_0000` (natively an address in the C
  library's startup) and a level beyond it 0 (natively unspecified). Keep? Levels above 0 follow the device's
  frames, which natively needs frame pointers kept and sibling calls off (plain native `-O2` crashed at level 1).
- **Calls through another prototype** (`329ef7a`; its independent testing pass `4409160`, sol: 2,880 native
  comparisons, 72 coherent argument values, 1,101 further runs and 27 corpus programs, no wrong result): every call through a function pointer now compares its target with
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
  something else? (historical compiler-rt experiment, since superseded by the decision above and `rt/libgcc`.)

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

## Independent testing of the SQL oracle, the checks, the entry-frame fix and the Lua probe (2026-11-17)

An Opus testing pass over four pieces sol built, looking for wrong results. Findings, each with a witness that
fails today:

- **The checks** (low), four gaps, each witness in `scripts/test_checks.py`:
  - the bare-time check read only `.py`/`.sh`/`.js`/`.ts` names, so an extensionless shell script (this tree's
    `.githooks/pre-push`) was never read. Fixed: scripts are also selected by their shebang line, and a
    shebang naming an interpreter the check has no scanner for is reported, not skipped; the witness is an
    ordinary test;
  - positional Python timeouts (`wait(30)`, `communicate(None, 30)`) were not flagged. Fixed: the positional
    duration of ten standard-library calls is checked (`POSITIONAL_TIMEOUTS` in `scripts/checks.py`); names
    whose positional argument is often not a duration (`dict.get`, `select`) stay keyword-only, a debt with
    the route of resolving the called object's type; the witness is an ordinary test;
  - a file that was not UTF-8 was not scanned for host paths. Fixed: every file is scanned, by bytes (as
    Latin-1, which keeps every ASCII byte) or decoded when it has a UTF-16/32 byte-order mark; the witness is
    an ordinary test, with a second for wide text and binary data. Not covered: UTF-16 without a byte-order
    mark;
  - an `amend!` subject (`git commit --fixup=amend:` or `reword:`) passed the WIP check on `main`. Fixed:
    `amend!` is refused like `fixup!` and `squash!`; the witness is an ordinary test.

  All four fixed on `wip/small-fixes`, one commit each; no witness is skipped any more. Every other claimed
  rejection was seeded and fails as documented (host paths, litter, symlinks, time literals, WIP ancestors, a
  bad non-HEAD revision, `_` and binding arms in the executor library, a removed attribute policy, clang 23 by
  `CLANG`, LLVM 23 by `LLVM_DIR`, the refused attributes). Not covered and not claimed: `matches!`/`if let`
  catch-alls, the experiment observers' standalone workspaces, a pushed revision that weakens its own
  `scripts/check.sh`.
- **The SQL oracle** (latent): the driver returns -1 for a key absent from the table, and the generated
  lookups truncated the result to one bit, which marked the row. No measured oracle reached it, because
  `scale.py` refused a truth table that was not all 0 or 1. Fixed on `wip/small-fixes`: the lookups of
  `scale.py`, `coherent.py` and `grover.py` mark a row exactly when the query returns 1 (`icmp eq i32 %v, 1`),
  the refusal is gone, and `edge/edge.py` runs all twelve edge queries, `abs_error`'s error row (-1) among
  them; with real SQLite all 61 edge checks pass, `abs_error` with six of eight rows marked (truncation marked
  seven). `an_absent_key_in_superposition_is_not_marked` (`front/tests/tests/sql_oracle_glue.rs`) fails on the
  truncating lookup (`experiments/sqlite-on-device/README.md`, "Independent testing pass"). The N=16 to 512
  scale runs were not rerun with the comparison (their tables have only 0 and 1, where both agree).
- **The Lua probe** (low): `%a` of a subnormal double is musl's spelling, not glibc's (question above;
  `front/tests/tests/printf_hex_float.rs`).

Found correct: the SQL oracle's marked sets, recomputed from the query semantics by host SQLite and by hand,
at every shape and size, and every device Grover amplitude at N=16 to 512 against a separate dense
calculation; an edge-row database (NULLs, duplicates, 64-bit bounds, collation, overflow, all and no rows),
all 56 checks (`experiments/sqlite-on-device/edge/`). The entry frame at every memory size around it, seven
signatures, three data layouts, four option sets, and with musl startup at every stack size near both edges,
against the independent reference interpreter of the time (removed on 2026-11-17) (`crates/exec/tests/entry_frame_boundary.rs`,
`front/tests/tests/entry_frame_musl.rs`). The Lua probe's three stages rerun, five more scripts (error
messages, format corners, garbage collection under pressure, integer/float edges, ordered tables) agreeing
and reversing exactly, a second predicate with three of eight marked, and every Grover amplitude against
closed forms (`experiments/lua-probe/README.md`, "Independent testing pass").

## Questions for the maintainer

Gone through on 2026-11-17 after the maintainer's decisions ("Resumed on 2026-11-17"); none is answered. Withdrawn,
with the machinery they concerned (`mark`, `rewind`, machine states in superposition, the call with a fault value;
git has their text): Choices made in machine-sites (2026-11-17), not yet asked; Independent reference snapshot
storage (2026-11-17); From the design seam and its council (2026-11-16, night; see "Design audition of
2026-11-16"); A rewind naming a mark that an outer rewind un-ran; Which fault a rewind gives when running again
would pass the budget; Superseded (2026-11-17): the leftover is now taken back; Time and the run's budget; How the
budget is required; The call with a fault value; Reference-only clock/diagnostic boundaries; A budgeted callee's
padded clock and its caller's mark; A mark's budget once its frame is left.

- **Which bytes hold qubits, for the fault `qubit_memory_access` (2026-11-17, step 3 of the work).** Taken
  (the simulator's rule already, the easiest to reverse): a byte holds qubits from the first gate that names
  any of its bits until that gate is undone, then for the rest of the run; not released when its qubits are
  definite again, when measured, or when its stack frame returns, and the rule is per whole byte (a load of a
  byte one of whose bits a gate named faults). Consequence: a stack byte once gated faults any later ordinary
  use of that address. Alternatives, each accepting more programs: a byte whose 8 bits were all measured goes
  back to memory holding the values found (a device knows that classically); or a release of qubits at an
  address, as `qfree` is for a register (a new instruction, so the maintainer's to decide). Recommendation:
  keep the rule until a front emits gates at addresses; then the measured-byte release.
- **Applied functions and qubits in memory (2026-11-17).** Taken, per the orchestrator's brief: inside `apply`
  the rule is the same, so an applied function cannot read qubits in memory; before, each run saw its
  configuration's values. A program copies the qubits into a register with controlled X gates, applies, and
  copies back (an expansion into gates and `apply`; the Grover test with x in memory does this). Question: is
  a derived form wanted that names bits at addresses among an `apply`'s arguments, defined by that expansion?
  Recommendation: yes once C functions take qubits through pointers.
- **Names left from the removed machinery** (2026-11-17): the journal-content observer's public API still
  counts and lists `apply`'s runs as `branches` (`ApplyJournal::branches`, `BranchJournal`, `branches_total`,
  `branches_present`), as does the front's test helper `branches`; `Stats::leftover_qubits` counts the columns
  the simulator keeps after a nonzero release; and the experiments' READMEs (`experiments/journal-content`
  most) still say "worlds". Taken: unchanged for now (the rename touches the observer, its classifiers, their
  tests and the experiments' READMEs); `RunError::RegionFault`'s count is `faulting` and its message says basis
  configurations. "Region" stays in fault and option names (`quantum_in_region`, `--region-budget`,
  `allow_region_faults`); `docs/DESIGN.md` defines it as the applied function. Recommendation: rename to
  `runs` / `configurations` / `kept_qubits` in a step that touches the observer anyway.
- **Fault codes 15** (`bad_rewind`, removed) **and 11** (`dirty_free`, removed when `qfree` became the partial
  trace): taken, left unused rather than renumbering, since codes are stable and a program may have measured
  one; the new `qubit_memory_access` took 17. Recommendation: keep them unused.
- **`printf("%a")` of a subnormal double (2026-11-17, the Opus testing pass):** the device prints musl's
  normalised spelling (`0x1p-1074`), the native reference glibc's denormal one (`0x0.0000000000001p-1022`);
  musl also rounds `%.3a` of `0x1.fffffp-1023` to `0x2.000p-1023`. Equal values, both conforming C; a native
  musl build prints what the device prints. Taken: unchanged, witness ignored
  (`front/tests/tests/printf_hex_float.rs`). Recommendation: follow glibc, as for a copied `sigjmp_buf`,
  by an overlay of musl's hexadecimal branch of `fmt_fp`, low priority; or record musl's spelling as the
  device's and drop the witness.
- **Open; changed by the work of 2026-11-17:** with `qfree` the partial trace, the option for the machine's
  meaning distinguishes only the stop at an `apply` faulting for some argument values, so
  `Options::machine_meaning` (`bennett run --machine-meaning`) now does exactly what `allow_region_faults`
  (`--allow-region-faults`) does. Taken (easiest to reverse): both kept, documented as the same. Recommendation:
  keep one; `--allow-region-faults` says what it does, `--machine-meaning` says why.
- **The NaN payload rule and the default NaN** (e7). Taken: two NaN results agree when each carries the
  payload of one of the tuple's NaN operands or the default NaN's, the sign free. The default NaN is not an
  operand of the helper, but it is an operand of the inner operation whose operand order differs (an inner
  `0/0`); without it 1,476 of 164,096 `__divdc3` tuples fail. Recommendation: keep. The fast grouped checks
  of `front/tests/tests/complex.rs` still count any two NaNs as equal (looser than the rule); route: give
  their comparison the operands, as `compare.py` has them, and apply the same rule.

## Questions the maintainer answered on 2026-11-16, in the afternoon (kept for their detail)

Each recommendation was taken ("Decisions the maintainer took on 2026-11-16, in the afternoon").

- **NaN payloads of the complex helpers.** Over 2,532,864 inputs the device's libgcc helpers differ from the native
  archive in 50,952 results, all in which input NaN's payload the result carries; nothing else differs, and both
  x87 helpers match completely. Measured cause: native code generation commutes `fadd`/`fmul` operands, and SSE
  propagates its destination operand's NaN; run natively in LLVM's operand order, every bit matches the device
  (`rt/libgcc/README.md`); the independent testing pass showed the other half: the same IR with one fixed
  operand order per instruction reproduces the archive on every tuple, and the host's GCC rebuilds the
  archive's six members exactly from the vendored source (`exact/commute.py`, `exact/archive.py`).
  C does not promise which input NaN's payload a result carries. Accept payload
  differences of this kind as agreement? Recommendation: yes, compared by a rule that allows exactly this. Taken
  meanwhile (easiest to reverse): the bit-for-bit requirement stays, recorded as a debt with its route (preserve
  the archive's per-operation operand order through helper compilation and SoftFloat lowering).

- **Controlled updates of ordinary memory and the coherent class.** The fallback oracle's testing pass asks whether a
  controlled in-place update of ordinary memory (`cells[0] += 2` under a qubit, as in its `nested` and `loop`
  programs) belongs to the documented coherent class; `docs/DESIGN.md` and `docs/MARKERS.md` limit the class to
  qubit variables. If the class grows, the oracle's weight-only checks would also accept a fallback that
  collapses such updates, so its checks must grow with it. Recommendation: keep the class as documented.
  Taken meanwhile: as documented.

## What is still refused, and what removes it

| Refused today | What removes it |
|---|---|
| Remaining runtime operations (locks, links, directory enumeration, block transport, permissions/metadata, image loader and allocation exhaustion) | Runtime debts below; step 1 above |
| A call through another prototype where x86-64 does not determine what is read: the upper half of a 32-bit value, a missing argument that is read, an integer from a floating-point register, a variadic floating-point argument without `%al`, a narrow argument that is not the extension the callee relies on (named fault `unreachable` when the call runs); a direct call whose argument differs from its parameter only in register class (native -O0 and -O2 disagree) | Nothing to remove: the native result is not determined (`docs/DESIGN.md`, "Calls through a prototype"). Conservative, removable by a finer model where named: a variadic floating-point argument after an unread named one, a call whose result is used only by dead code before the optimiser, values in memory read at another place |
| A qubit-dependent call of a function that uses `setjmp` | The front's own effect summary of `setjmp` (writes only its buffer) when deciding what runs coherently (`docs/DESIGN.md`, "Non-local jumps") |
| `__float128`; `powi` on `_Float16`/`__bf16`; `#pragma STDC FENV_ACCESS` (with it musl's `ilogb`/`ilogbl` in the corpus accuracy checks; libgcc's `__divxc3` itself works); non-default rounding modes and floating-point flags | Step 5 above |
| A store under qubit control; an update with no known inverse; a value needed after it changed; qubit variables in structs, heap objects or passed to functions; gates in a loop under qubit control that LLVM does not unroll fully | For stores and calls whose writes stay inside a whole applied function: step 6 of "Resumed on 2026-11-17"; for gates in a loop with an ordinary trip count: its plan ("Known weak spots"). The rest is open: keeping what ordinary code overwrote under a qubit beyond the `apply` (`docs/DESIGN.md`, "Open questions") |
| A qubit-dependent value used by ordinary code or printed; measurement under control | Channels |
| Non-empty inline assembly; code without source | An emulator of the real machine, compiled as an ordinary program |
| A floating-point operation that reads the subnormal mode (arithmetic, comparison, conversion from floating point, intrinsics other than `fabs`/`copysign`/`is.fpclass`) in a function promising a non-IEEE mode (`-fdenormal-fp-math=preserve-sign`, `positive-zero`, `dynamic`, or the `-f32` form); moves and sign/magnitude bit operations there are accepted | The floating-point environment carried through SoftFloat's state (flush inputs and outputs as the promised mode says), compared with native code run under MXCSR DAZ/FTZ |

## C North Star rungs 1–3 (measured 2026-11-16/17)

**Since 2026-11-17 the second implementation is gone**: the independently written reference interpreter
modelled `mark`, `rewind` and machine states in superposition and was removed with them, so rungs 1 and 2 are
checked on `bennett-exec` against native only; the "independent" figures below are the measurements of
2026-11-16/17. Debt: "Resumed on 2026-11-17".

`experiments/northstar-ladder/README.md`, `front/tests/tests/ladder.rs`: unchanged archived five Int8
functions and 258-line hashtable, provenance checked against pinned Git blobs. Rungs 1/3 measured at `5361c71`,
after the file-runtime rebase; rung 2 repeated on 2026-11-17 against production `bd2e384` with persistent
independent snapshots. **The C portion of all three rungs passes as worded.**

Scalar domain: 131,840 tuples per compiler input, no exclusions, native O2 and O0/UBSan agree. Across
clang O0/O2 and both production modes: 527,360 native comparisons, 263,680 exact resets, 9,320,952
forward steps, zero faults (118.233 s). The independently written reference interpreter added 263,680
native comparisons and reversals of its own runs, zero faults (1,278.459 s). These slow sweeps are
explicit experiments; two modes of one executor are not counted as two implementations.

The original table's basic/grow at n=0,1,7,64,1000 passes all 40 full i64 production comparisons, native
O2 versus O0/ASan/UBSan, 195 console bytes/status 0, and two exact full-memory resets. O0/O2:
804,215/803,622 steps, 15,994,967/16,000,182 journal bytes; both images 2,108,912 memory bytes.
The independent interpreter also completed all ten full native results from both inputs and reversed its
own runs byte-exactly, with production state/step comparisons at checkpoints, completion and reset.
Its isolated release process passes in 41.091 s under the unchanged 120 s / 1 GiB cap. Before the change,
a fresh profile attributed 68.76% of sampled CPU time to slot copying/iteration/clone, with a 1,024-record
history clone costing 9.301 ms versus 11.538 us for the current state. `rpds` persistent history, slots,
frames, tape and byte map plus standard `Rc` cold metadata close the gap; the sharing regression failed
on the old model. No executor disagreement was found. No input was sampled or removed.

Musl hello: 13 exact native bytes/status 0, three identical same-executor replays and four full resets
at each clang level. O0/O2: 3,379/2,917 steps, 68,371/58,357 journal bytes,
2,108,896/2,106,160 full reset memory bytes. No incorrect C result was observed. These fresh receipts
supersede the early pre-file-runtime numbers. The runner now accepts rung 2; future resource stops
remain UNMEASURED and nonzero.
Final repository checks after rebasing on `bd2e384`: cargo test 471 passed/20 ignored, full frontend
182 passed/26 ignored, clippy and formatting clean; workspace-library clippy, tree guards and exact
discovery of all five ignored acceptance stages also pass. The measured 118-second scalar sweep is explicit experiment work, keeping it out of
the ordinary frontend run without changing its domain or assertions.

## Known weak spots

- **Two findings of the front audit of 2026-11-17 on loops, open** (planned with step 4 of "Next steps"). (1)
  The front writes one `apply` per update of a qubit variable, so `for (i = 0; i < n; i++) y ^= f(q, i) & 1;`
  is `n` applications, each with its own budget and fault register, where one `apply` of the whole loop's XOR
  with one budget would do; a loop inside a function that only reads memory, called with arguments that depend
  on a qubit, already runs under one `apply` and one budget. (2) Gates in a loop under a qubit condition are accepted only when
  LLVM unrolls the loop fully (`if (q) for (i = 0; i < n; i++) bennett_x(&y);` is refused; with `n` constant it
  is accepted). Route for an ordinary trip count: compute the condition once into a fresh qubit, keep the
  ordinary loop with each gate controlled by it, clear it after. A trip count that depends on a qubit would make
  the number of gates depend on a qubit; what to do then is a question for the maintainer once the plan is
  reported.

- **Three wrong translations in the front, found by the front audit of 2026-11-17, fixed** (`8bff6a5`,
  `313386d`, `bc5b3cb`; each a case of `what_was_once_translated_wrongly_is_refused_by_name_in_every_mode` in
  `front/tests/tests/refusals.rs`, run at O0, O2 and with `--taint-all`, each seen accepted by the front before
  its fix). (1) A gate whose condition is a conjunction of single qubits took them as controls without the
  check that each was unchanged since it was read (`_Bool old = q; bennett_x(&q); if (old) bennett_x(&y);`
  gave y = 0, C gives 1): both paths now share one check and the program is refused at the read. (2) A
  qubit-dependent atomic update or volatile or atomic read whose result was unused vanished from the device
  program (`__atomic_fetch_add(&cell, (int)q, ...)` left cell 0): every qubit-dependent instruction other than a
  call that has an effect on memory is now refused by name. (3) A `setjmp` in a function with qubit variables:
  a `longjmp` resumes the function along no control-flow edge, so the qubit facts took a read after the
  `setjmp` for the first return's value (0 where C gives 1): refused by name at the `setjmp`. Debt with its
  route (`docs/DESIGN.md`, "Non-local jumps"): resumption edges from every later call and `longjmp` in the facts,
  the unchanged-between checks and the cycle test.

- Independent fp-front testing added two software-rounding witnesses: `lroundl` and `llroundl` on
  unsupported x87 exponent `0x3fff`/significand zero returned INT64_MIN on the device, versus glibc's 0,
  at both `-O0` and `-O2`. **Fixed on 2026-11-17:** the runtime's `lroundl` and `llroundl` follow glibc's
  software bit operations, and the front calls them for long double (`rt/softfloat/README.md`); both
  witnesses run by default and the 132 excluded tuples are compared (all agree).
  A third ignored witness records a native-reference divergence: LLVM 18's optimized bfloat signbit
  returns zero even for -1, while the device and native O0 return the correct 1. This is not evidence
  of an incorrect device sign bit. All three were explicitly run and failed; no production fix in
  this testing pass. The fast pipeline probes retain 4,224 exact powi results, 17,292 rounding
  results (17,028 before the lroundl fix, the rest excluded) and 2,430 classification results across both
  optimization levels, with only the precisely identified bfloat tuples excluded. File/stdin/output admission probes pass. Full scope, counts,
  reproduction and limits: `rt/softfloat/README.md`, "Independent fp-front follow-up".

- Independent conformance testing of the noncanonical x87/input fixes (`front/tests/tests/p0_independent.rs`):
  its five wrong results were fixed on 2026-11-17 and their witnesses run by default: `powi(x, 1)` returns
  noncanonical encodings unchanged, as libgcc does; `roundevenl` follows glibc's software bit operations on
  them (every sign and exponent compared natively); `is.fpclass` follows LLVM's x86-64 expansion mask by
  mask (`isfinite`/`isnan` at `-O0`; all 1,024 masks compared natively, `fp_class.rs`); and, beyond the
  pass's list, `roundl` follows glibc's software bit operations as `roundevenl` does. The documented half and
  bfloat software conversion differences have minimal ignored witnesses, run and observed failing.
  Exact coverage and excluded cases: `rt/softfloat/README.md`, "Independent conformance pass".
- Front input admission (`front/tests/tests/p0_invalid_input.rs`): since 2026-11-17 an ordinary parse
  failure is named "the input is not valid LLVM IR" after LLVM's diagnostic (status 2 as before), and an
  empty input is accepted as the empty module it is in LLVM (`llvm-as` and the verifier accept it; the
  testing pass's witness asked for a refusal, which no document requires), becoming an empty device
  program; a signal inside LLVM's reader (a 44-byte bitcode module with dangling subprogram type metadata
  made its metadata loader segfault) is caught and named the same way, status 2, no device program. A crash
  in the reader on valid input, an LLVM bug, would be named so too: the two cannot be told apart.

- The changed-line mutation scope preview can crash on a diff without generated workspace Rust
  mutants: `python3 scripts/mutate-diff.py origin/main --list` on the Lua probe reports
  `No mutants to filter` then `JSONDecodeError`, because cargo-mutants emits empty stdout.
  Route: handle that output before JSON parsing; standalone experiment workspaces still need their
  own mutation checks (`experiments/lua-probe/README.md`). No mutation pass is claimed for it.

- Removed on 2026-11-17: the fallback oracle compared the four `cells` bytes and the qubit variables only, so
  a stray write just past the array went unseen by both its native and device sides. The fixture's cells now
  lie in a twelve-byte image between guard bytes, and every native and device comparison takes the whole
  image; seeded stray writes before and past the cells are rejected on both sides
  (`front/tests/c/fallback_oracle/README.md`, "Guard bytes").

- A stale `setjmp` record (its frame returned) can match another activation of the same function at the same
  depth and stack address and jump into it (C leaves this undefined; decided 2026-11-16: today's behaviour kept).
  Route if it is ever to fault: an activation counter in the jump record.

- **A partial `sigjmp_buf` copy can select the wrong continuation** (native glibc and musl status 0,
  device 1, empty stdout, at both O0/O2): copying the first 16 bytes between two live savemask-0
  environments preserves the native target's instruction pointer at bytes 56..64, but replaces the
  device continuation at bytes 8..16. Ignored witness, explicitly seen failing:
  `a_partial_record_copy_preserves_the_glibc_continuation` in `front/tests/tests/sigjmp_copy.rs`;
  no production code changed. Route: preserve glibc's byte-level copy semantics in the lowered C
  environment separately from the device record, then enable the witness and sweep more prefix/suffix
  boundaries. The broader copied-buffer evidence hole is narrowed: 576 cases at each of O0/O2 compare
  the musl-linked device with native glibc and native musl (memcpy, struct assignment, two union paths,
  copies in pieces, record-only/record-plus-flag/mask-only partial copies, savemask 0/1/-7, original
  re-saved or not, original/copy targets and changed masks). All device stdout/status matches glibc,
  including the 94 O0 / 92 O2 cases where musl differs. Native musl changes 56 rows between levels;
  glibc and device are stable. Full reproduction and limitations: `rt/musl/README.md`, "Copied jump
  buffers"; retained heavy test and `rt/musl/test-sigjmp.py`. Returned-frame jumps are excluded, as decided.

- Return-address values at and beyond the outermost frame (`0xFFFF_FFFF_0000_0000`, then 0) are kept as they are
  (decided 2026-11-16); native gives other values there, so a program printing them differs from native.

- The exit status is not masked on the device (decided 2026-11-16); native masks it to 8 bits, so corpus
  comparisons compare the masked value.

- Debt: equal-priority constructor order across separately compiled objects follows a native build of the linked
  module, not separately linked objects (C leaves it unspecified). Route: lower startup per unit before linking.

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
  native build crashes). `bennett-front` names such input as not valid LLVM IR, with the verifier's finding and
  status 2 (`front/tests/tests/invalid_input.rs`); it aborted inside LLVM's bitcode reader before when the input
  carried debug info, since the reader verifies while upgrading debug info and stops with a fatal error. A
  fatal-error handler around reading now names the input instead.
- A struct passed by value from a call clang marked `tail` lost the stores that fill its copy once `byval` was
  dropped (a silent wrong result at `-O2` when the callee is not inlined; fixed with the over-aligned variadic
  work, `front/tests/c/byval_tail_call.c`). The front's other lowerings that pass a copy by address (the variadic
  area, values in memory) already cleared the mark.
- A function that uses `stackrestore` cannot run coherently yet.
- A null dereference does not fault.
- `bennett run --arg` refuses arguments for parameters wider than 64 bits (`Executor::new` takes `u64`).
- Floating point: which NaN comes back when several operands are NaNs is unspecified; a `volatile` vector is
  refused. `long double` encodings no arithmetic produces (pseudo-denormals, unnormals, pseudo-infinities,
  pseudo-NaNs) are read as x87 reads them in every operation since 2026-11-16, at 4 to 5 device steps per
  operand (+10.0% to +11.7% steps on the long-double tests, `rt/softfloat/README.md`); glibc's `fmal` and
  libgcc's truncations to `_Float16`/`__bf16`, software natively, read them at face value and still differ
  there (route if that matters: follow each routine's bit tests on such operands, as `roundl` and
  `roundevenl` now do). Where C
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
- Nothing private goes into the tree.
- Since 2026-11-15 (maintainer: land the work and push regularly): each job works in its own checkout of this
  repository, runs `cargo test`, clippy, fmt and `front/test.sh`, rebases on `main` and pushes each finished step
  to `main`; unfinished work goes to a `wip/` branch, never to `main`. A persistent advisor holds the picture and
  launches one job per goal; launches stop when the quota's pace is reached.

## Files, clocks and entropy: implementation and remaining debts (2026-11-16)

The carried `bennett-runtimes` VFS, syscall dispatch and clock/entropy models now run through musl on
this device (`rt/musl/vfs.c`, `devices.c`, `models.c`). No executor change. Inputs enter the run image
with `link.py --clock-start N --entropy-image bytes.bin --file-image files.json`; defaults zero/empty.
The clock is unsigned nanoseconds, start plus read count modulo 2^64. Entropy is the exact byte tape,
shared across getrandom/getentropy/random device files, with exhaustion before mutation. File image
installation, file state and both consumption counters are ordinary memory and reverse with the run.
`test-devices.py` compares determined native file results and an independent native clock/tape oracle,
including wrap, and checks exact reset after full executor reversal: all 16 O0/O2 runs pass (62.76 s).
Representative costs: files 75,617 steps / 1,558,865 journal bytes; constructor image 40,503 / 817,527;
clock and tape 30,758 / 670,118; exhausted empty tape 2,101 / 41,781 (a fully reversed named fault).
Existing musl integration tests pass (37 comparisons). The targeted GCC temporary-file cases exposed
`tmpnam`'s use of `readlink` to check a missing name; readlink/readlinkat now return ENOENT for a missing
path and EINVAL for a non-symlink. A native/undo temporary-file regression passes at 63,073 steps /
1,266,897 journal bytes, including anonymous tmpfile and its descriptor flags.
All six previously file-blocked corpus programs now agree with native (`experiments/c-corpus/README.md`):
GCC `printf-2` / `user-printf` / `fprintf-2` 58,353 / 58,416 / 57,717 steps at `9194605`; the earlier
file implementation already passed `loop-2f` / `loop-2g` at 35,235 each and c-testsuite `00187` at 102,998.
No whole corpus was rerun. Four checks pass after rebasing on `40c84d4`: cargo 306 tests, front 170,
clippy all targets with warnings denied, formatting clean.

Its independent testing pass (other model family, 2026-11-16) compared about 250 file, clock and entropy
calls with native glibc on ext4 and found 18 divergences, all in the file model; clock and entropy values
matched the decision in every probe. Wrong state: `rmdir(".")`/`rmdir("d/.")` removed the directory;
`rename(".", x)` moved it; `rename(d, "/")` and `rename(d, "..")` replaced the root; `rename(file, "new/")`
moved the file; `open(dir, O_RDONLY|O_TRUNC)` succeeded; a working directory removed and remade under its
name took relative creates into the new directory. Refused where Linux succeeds: `fsync` of a directory,
`O_ACCMODE` opens, `SEEK_DATA`/`SEEK_HOLE`, `fchdir` to a removed directory, the caller's CPU-time clock
ids, and partial vectored IO with a bad later element. Wrong error: unenforced `NAME_MAX`/`PATH_MAX`, and the
error order of `ftruncate`, `truncate`, `access`, `pread`/`pwrite`, `mkdir("file/")`, `open("missing/f/",
O_CREAT)` and renames onto `.`. All fixed; 8 self-checking witnesses (`rt/musl/tests/devices-regress/`)
return 0 natively, failed before, and un-run exactly from their end and three mid-run cuts. `test-devices.py`
24 passes (147.31 s); the fixtures cost up to 6% more steps (files 79,057 / 1,632,977 journal bytes).

Debts, with routes (the goal is not narrowed to the present surface):

- **Separate per-invocation inputs**: images are supplied while linking the run image. Add a symbol/relocation
  map plus an initial-memory loader, with mutable input bindings and before-optimisation preservation, to
  rebind images without relinking. Keep the same clock/tape semantics and reset tests.
- **File operations not carried**: locks, hard/symbolic links, directory enumeration, extended attributes,
  renameat/linkat variants, file-backed/shared mmap and other unmapped calls trap. Extend this boundary over
  ordinary memory and
  compare each operation and error path with native. A lock owner/table must be real state, not an accepted
  no-op; a directory iterator needs stable entries/cookies; symlinks need component-wise resolution.
  File mappings need inode-backed pages and address aliases with a journalled mapping table, checked by
  writing through each alias and through file descriptors before/after unmapping.
- **Namespace and metadata**: the model has root ownership, a fixed device identity, zero timestamps and
  no credential/ACL checks. Carry explicit credentials and metadata inputs, and update inode times from
  the input clock under a documented rule. The independent testing pass (`rt/musl/tests/devices-regress/`)
  fixed the ordering, dot-name, trailing-slash and working-directory lifetime results it found. Still open:
  a removed working or descriptor directory names nothing, so `.`/`..` relative to it give `ENOENT` where
  Linux still resolves them (route: resolve relative names from the inode, keeping a parent link per
  directory inode); directory sizes are 0 (ext4 reports blocks); there is no descriptor limit (natively
  `EMFILE` at `RLIMIT_NOFILE`, an environment input not yet modelled). Directory links, size, sparse
  pages, file modes/umask, append and descriptor offsets already have native comparisons.
- **Allocation exhaustion and reclamation**: allocations use the existing finite musl arena. VFS allocation
  failure is a named fault; syscall-wide ENOMEM rollback and reclaiming unlinked/closed names/inodes/open
  descriptions remain. Stage allocations before publication, maintain inode/description references, and
  compare failure state byte for byte. Full un-run already restores all allocation and filesystem state.
- **Block transport and sleeping**: the archive block backend is not carried without an image consumer;
  no block nodes claim to exist. Carry its preserved image/sparse sector overlay with an explicit input.
  nanosleep/timers still trap; define sleep against the input clock (whose advance is currently reads only)
  before adding it. No host time or random source is read by the device.
## IR and CLI mutation pass (2026-11-16)

**Defect:** an overflowing rotation angle can be read but cannot be read back after writing; the ignored
witness explicitly failed on the unchanged production code. Fixed later the same day: the reader refuses an
angle that is not finite as an `f64` (`docs/SYNTAX.md`), and the witness runs by default.

Production base `3418e51`, cargo-mutants 27.1.0; `crates/ir` excluding `fault.rs`, and `crates/cli`.
Added 21 ordinary tests and one ignored witness in `crates/ir/tests/mutation_contract.rs` and
`crates/cli/tests/mutation_contract.rs`, with a small CLI fixture. The original per-package screen gave
362 caught, 88 missed, 70 unviable and 5 timeouts. New tests caught 75 of those misses; the remaining
13 were checked against all default Rust packages, with two further diagnostic gaps caught: an alias
error must identify the uniquely duplicated register, rather than blame an unrelated one. These are
package-screen improvements, not a claim that all 77 were previously missed by the executor suite.

Final: **439 caught, 11 equivalent survivors, 0 unexplained survivors, 70 unviable,
5 timeouts**, 525 unique mutants. Repeated-stage trials are counted once by exact mutant name.
The timeouts are non-progress loops: the memory/data/function parsers made no-ops, `Cursor::eat`
always true, and its position update changed from addition to multiplication.

| File (under `crates/`) | Caught | Survived* | Equivalent | Timeout | Unviable |
|---|---:|---:|---:|---:|---:|
| `cli/src/main.rs` | 2 | 0 | 0 | 0 | 1 |
| `cli/src/run.rs` | 41 | 0 | 3 | 0 | 1 |
| `ir/src/lib.rs` | 0 | 0 | 0 | 0 | 1 |
| `ir/src/module.rs` | 59 | 0 | 1 | 0 | 5 |
| `ir/src/reader/cursor.rs` | 46 | 0 | 0 | 2 | 4 |
| `ir/src/reader/error.rs` | 8 | 0 | 1 | 0 | 1 |
| `ir/src/reader/lexer.rs` | 17 | 0 | 0 | 0 | 7 |
| `ir/src/reader/literal.rs` | 27 | 0 | 1 | 0 | 1 |
| `ir/src/reader/mod.rs` | 12 | 0 | 0 | 3 | 9 |
| `ir/src/reader/names.rs` | 12 | 0 | 0 | 0 | 0 |
| `ir/src/reader/statement.rs` | 45 | 0 | 0 | 0 | 24 |
| `ir/src/types.rs` | 20 | 0 | 0 | 0 | 2 |
| `ir/src/validate/error.rs` | 0 | 0 | 2 | 0 | 0 |
| `ir/src/validate/function.rs` | 99 | 0 | 1 | 0 | 10 |
| `ir/src/validate/mod.rs` | 17 | 0 | 0 | 0 | 4 |
| `ir/src/writer.rs` | 34 | 0 | 2 | 0 | 0 |
| `ir/src/ops.rs` (macro definitions) | 0 | 0 | 0 | 0 | 0 |
| **Total** | **439** | **0** | **11** | **5** | **70** |

*Survived excludes the separately classified equivalents. `ops.rs` yielded no generated candidates: the
tool does not expand its mnemonic macro. The explicit seeded expansion pass below closes the measured
gap for 19 semantic defects; it does not change this generated-mutation table. Neither set is a claim
of complete mutation coverage. `fault.rs` belongs to the separate executor mutation pass.

Reproduce the final test population and classifications from this test revision with:

```sh
for package in bennett-ir bennett; do
  CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=1 cargo mutants -p "$package" \
    --test-package "$package" --exclude crates/ir/src/fault.rs \
    --jobs 2 --jobserver-tasks 2 --timeout 60 \
    --output ".tmp/mutants-$package" \
    --cargo-arg=--config=profile.dev.package.bennett-exec.opt-level=1 \
    --cargo-arg=--config=profile.dev.debug=0
done
```

With the final tests, this per-package loop reproduces the table. Exit 2 is expected for the listed
equivalent survivors. For full-default-Rust qualification, select those exact names with `--re` and
use `--test-package bennett-ir --test-package bennett --test-package bennett-exec`, executor opt-level 3
and `--timeout 180`; the measured pass did so before classifying the remainder.

The measured pass screened each package separately at executor opt-level 1, rechecked its misses with
new tests, then qualified the remainder with all three default Rust packages at opt-level 3. Debug info
was off, test threads 1, two persistent mutation workers; initial/recheck test timeouts 60 seconds,
full qualification 180. The five timed-out mutants cannot advance their parser loops at either limit.

Every remaining survivor (locations refer to the unchanged production base):

- `crates/cli/src/run.rs:137:20: replace > with >= in <impl std::fmt::Display for Timed>::fmt`: Only an exact-zero elapsed duration changes rendering. Zero-duration throughput has no specified representation; execution, counters and every positive-duration report agree.
- `crates/cli/src/run.rs:50:62: replace < with <= in entry_arguments`: Only zero changes its sign flag. Both literal conversions produce the same canonical zero.
- `crates/cli/src/run.rs:65:37: replace | with ^ in report_outcome`: A canonical result has no bits above its width; the sign-extension bits and value bits are disjoint, so OR and XOR agree.
- `crates/ir/src/module.rs:250:26: replace | with ^ in code_address`: The shifted function address and the masked low-32-bit position have disjoint bits, so OR and XOR agree.
- `crates/ir/src/reader/error.rs:34:9: replace <impl fmt::Display for NameKind>::fmt -> fmt::Result with Ok(Default::default())`: Only the unspecified textual spelling of a name kind in a diagnostic disappears. Its structured kind, name, location and rejection are unchanged.
- `crates/ir/src/reader/literal.rs:28:17: replace match guard digits.bytes().all(|b| b.is_ascii_digit()) with true in parse_integer`: The public lexer forbids an initial plus sign; standard integer parsing still rejects fraction/exponent tokens. Removing the decimal digit guard changes no readable integer.
- `crates/ir/src/validate/error.rs:58:9: replace <impl fmt::Display for Location>::fmt -> fmt::Result with Ok(Default::default())`: Only unspecified diagnostic formatting changes; the structured location, problem and rejection remain identical.
- `crates/ir/src/validate/error.rs:87:9: replace <impl fmt::Display for Expected>::fmt -> fmt::Result with Ok(Default::default())`: Only unspecified diagnostic formatting changes; the structured expected type, problem and rejection remain identical.
- `crates/ir/src/validate/function.rs:452:17: delete match arm (Some(found @Type::Qreg(_)), Type::Int(width)) in Checker<'a>::call_arguments`: A wrong-width register still fails the ordinary integer-parameter check. The diagnostic gives the declared integer type rather than also listing its permitted register coercion; accepted calls are unchanged.
- `crates/ir/src/writer.rs:247:13: delete match arm b'\n' in write_string`: The fallback emits the same newline byte as a hexadecimal escape. SYNTAX permits both spellings.
- `crates/ir/src/writer.rs:248:13: delete match arm b'\t' in write_string`: The fallback emits the same tab byte as a hexadecimal escape. SYNTAX permits both spellings.

These equivalences use the documented acceptance/text contract; they do not pin unspecified diagnostic
prose or a zero-duration timing spelling. False diagnostic payloads are tested: the alias cases require
the actual duplicated register. The CLI byte-per-step check uses a two-step program, since dividing
and multiplying by one would otherwise agree. Numerical wall-clock throughput is not gated.

Validation: `cargo test` **313 passed / 20 ignored**, `cargo clippy --all-targets -- -D warnings` and
`cargo fmt --check` passed; `front/test.sh` **158 passed / 12 ignored**. The front result is retained
from the unchanged production/frontend source; subsequent changes only added or refined IR/CLI tests.
The ignored rotation witness was run explicitly and failed as expected. Region tests derive
`Fault::CODE_WIDTH`; the register-measure CLI fixture must migrate with the planned address form.

### Explicit macro-expansion seeds (2026-11-17)

Base `ed3f15b2f9ae30c20188110b041dd9f315621d50`. Each trial used a fresh scratch copy of that
revision, one defect at a time. The five `mnemonic_enum!` invocations in `ops.rs` were expanded manually
using the macro's own enum/`ALL`/`mnemonic`/`from_mnemonic` template; production source was never edited.
The unmodified expanded control passed **70 IR / 20 CLI tests**. The same control plus the new test
file passed **76 IR / 20 CLI tests**. The expansion is a control for these seeds, not a Rust compiler
expansion audit.

Before the new tests: **19 viable seeds, 10 caught / 9 missed**, no compile failures or timeouts.
After adding six tests in `crates/ir/tests/mnemonic_contract.rs`: **19 caught / 0 missed**. All nine
former misses were explicitly seen failing with the added tests. Both complete package suites ran
for every seed, using `--no-fail-fast` so a failing unit-test binary did not skip integration tests.
Every CLI run passed its 20 tests, before and after: the new catches below belong to IR. This is
package-level evidence, not a claim that the old executor suite missed all nine defects.

The added tests independently specify all 36 mnemonic-to-variant mappings and complete `ALL` lists;
assert typed reader instructions with unequal operands and nonzero angles; construct writer inputs
without reading the operation under test; retain two distinct gate controls; and reject malformed
operand arities. A consistent spelling swap can satisfy the former self-round-trip test, which
explains the missed swaps. Absent vocabulary/arity witnesses explain the other misses. The new
writer checks use literal expected text rather than the reader as their oracle.

New failing-test abbreviations (all in `mnemonic_contract.rs`):

- B: `binary_words_name_the_documented_operations`
- P: `predicate_words_name_the_documented_comparisons`
- C: `cast_words_name_the_documented_width_changes`
- F: `fixed_gate_words_name_the_documented_gates`
- R: `rotation_words_name_the_documented_axes_and_preserve_angles`
- A: `macro_named_instructions_require_the_documented_operands`

| Single seeded defect | Existing IR tests that failed before additions | Added IR tests observed failing |
|---|---|---|
| Expanded `BinOp::Lshr` / `Ashr`: swap `lshr` / `ashr` writer spellings | **missed** | B |
| Expanded `BinOp::ALL`: omit `Ashr` | **missed** | B |
| Expanded `BinOp::from_mnemonic("ashr")`: return `Lshr` | `ops::tests::mnemonics_round_trip` | B |
| Expanded `Predicate::Sgt` / `Sge`: swap `sgt` / `sge` spellings | **missed** | P |
| Expanded `Predicate::ALL`: omit `Sge` | **missed** | P |
| Expanded `Predicate::from_mnemonic("sge")`: return `Sgt` | **missed** | P |
| Expanded `CastOp::Sext` / `Trunc`: swap `sext` / `trunc` spellings | `a_handle_is_used_only_by_rewind`, `casts_and_ssa_have_strict_boundaries`, `examples_are_valid_and_survive_a_round_trip_through_the_writer` | C |
| Expanded `CastOp::ALL`: omit `Trunc` | `a_handle_is_used_only_by_rewind`, `casts_and_ssa_have_strict_boundaries`, `examples_are_valid_and_survive_a_round_trip_through_the_writer` | C |
| Expanded `CastOp::from_mnemonic("trunc")`: return `Sext` | `a_handle_is_used_only_by_rewind`, `casts_and_ssa_have_strict_boundaries`, `examples_are_valid_and_survive_a_round_trip_through_the_writer` | C |
| Expanded `FixedGate::T` / `Tdg`: swap `t` / `tdg` spellings | **missed** | F |
| Expanded `FixedGate::ALL`: omit `Tdg` | **missed** | F |
| Expanded `FixedGate::from_mnemonic("tdg")`: return `T` | `ops::tests::mnemonics_round_trip` | F |
| Expanded `Rotation::Ry` / `Rz`: swap `ry` / `rz` spellings | **missed** | R |
| Expanded `Rotation::ALL`: omit `Rz` | `gates_at_addresses_read_write_and_validate` | R |
| Expanded `Rotation::from_mnemonic("rz")`: return `Ry` | `gates_at_addresses_read_write_and_validate` | R |
| Binary writer: emit only mnemonic, width and lhs, omitting comma/rhs | `examples_are_valid_and_survive_a_round_trip_through_the_writer`, `measure_at_an_address_reads_writes_and_validates`, `well_formed_programs_are_accepted` | B |
| Binary reader: make comma/rhs optional, using lhs for a missing rhs | **missed** | A |
| Rotation writer: emit `0.0` instead of the stored angle | `every_readable_rotation_round_trips_even_when_validation_rejects_it`, `gates_at_addresses_read_write_and_validate` | R |
| Gate writer: iterate `controls.iter().take(0)` | `examples_are_valid_and_survive_a_round_trip_through_the_writer`, `gates_at_addresses_read_write_and_validate` | F, R |

For each expanded enum, the swap changes only the two explicit `mnemonic()` match-arm string
literals; the omission removes only the final element of `ALL`; the reader defect inserts an early
return for the named keyword and otherwise leaves lookup unchanged. The four other edits change only
the named reader/writer expression, retaining the unmodified expanded vocabulary. All seeds were
compiled and tested separately, never composed. The source under test was restored from the same
archive between trials. Reproduction outline, from the repository root:

```sh
repo=$PWD
mkdir -p "$repo/.tmp"
scratch=$(mktemp -d "$repo/.tmp/mnemonic-seed.XXXXXX")
git -C "$repo" archive ed3f15b2f9ae30c20188110b041dd9f315621d50 | tar -x -C "$scratch"
# Expand the five invocations using the mnemonic_enum template; run the unchanged control first.
# Apply exactly one edit from the table, then run both complete package suites:
(cd "$scratch" && CARGO_BUILD_JOBS=3 cargo test -p bennett-ir --no-fail-fast)
(cd "$scratch" && CARGO_BUILD_JOBS=3 cargo test -p bennett --no-fail-fast)
# Add only the new tests to this same seed and repeat both commands:
cp "$repo/crates/ir/tests/mnemonic_contract.rs" "$scratch/crates/ir/tests/"
(cd "$scratch" && CARGO_BUILD_JOBS=3 cargo test -p bennett-ir --no-fail-fast)
(cd "$scratch" && CARGO_BUILD_JOBS=3 cargo test -p bennett --no-fail-fast)
```

Measured with the default development profile, `CARGO_BUILD_JOBS=3`, headless environment and a
local temporary directory; each build/test held the shared build slot. No production code changed.
These 19 seeds do not enumerate every possible expansion or parser/writer defect; broader automatic
mutation of macro expansions remains a tooling debt, with explicit seeded trials as the current route.

Validation after rebasing on `eec811a`: `cargo test` **474 passed / 21 ignored**; `front/test.sh` **181 passed / 27 ignored**; `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` passed. The front also passed its C++ header and 88 LLVM-attribute checks.

## Vendored musl authentication (2026-11-17)

The upstream musl 1.2.5 release's detached signature verifies with the maintainer key downloaded from
`https://musl.libc.org/musl.pub`: GnuPG `VALIDSIG` fingerprint
`836489290BB6B70F99FFDA0556BCDB593020450F`, identity `musl libc <musl@libc.org>`, signature dated
2024-03-01 02:08:50 UTC. Trust in the key's identity comes from upstream HTTPS, not a local web of trust.
The authenticated archive's SHA-256 is
`a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4`.
All 2,697 vendored regular files, executable flags, entry types and 235 directories match; no extra or
missing entries. The former weak spot is closed. No vendor source changed; only plain downloads were made.

Exact authentication and content-comparison commands, from the repository root:

```sh
auth=$(mktemp -d)
curl --fail --location https://musl.libc.org/releases/musl-1.2.5.tar.gz -o "$auth/musl-1.2.5.tar.gz"
curl --fail --location https://musl.libc.org/releases/musl-1.2.5.tar.gz.asc -o "$auth/musl-1.2.5.tar.gz.asc"
curl --fail --location https://musl.libc.org/musl.pub -o "$auth/musl.pub"
mkdir -m 700 "$auth/gnupg"
gpg --homedir "$auth/gnupg" --batch --import "$auth/musl.pub"
gpg --homedir "$auth/gnupg" --batch --status-fd 1 --verify \
  "$auth/musl-1.2.5.tar.gz.asc" "$auth/musl-1.2.5.tar.gz"
sha256sum "$auth/musl-1.2.5.tar.gz"
tar -xzf "$auth/musl-1.2.5.tar.gz" -C "$auth"
diff --recursive --no-dereference "$auth/musl-1.2.5" rt/musl/musl-1.2.5
```

The additional entry-type/executable-flag comparison is reproduced in `rt/musl/VENDOR.md`.

## Independent checks of apply, removal and front fixes (2026-11-17)

`current_model_independent.rs` checks one runtime budget across nested loops and a callee against
hand-counted costs (5, 12, 29, 56 steps), with complex amplitudes, correlated dirty targets and an untouched
reference register: 28 runs, 896 configuration amplitudes checked within 1e-12. Four deep-callee fault
cases check private memory and console isolation. Ordinary calls, stack changes, console effects, gates
at addresses and apply reverse to every saved predecessor; measurement refuses reversal for both address
and register forms. Sixteen seeded address-gate/measurement runs have the analytic value and phase.

IR tests reject obsolete instructions in unused functions and qubit arguments to direct and indirect
ordinary integer calls; writer and CLI checks cover the remaining forms. Front variants exercise four
save/jump spellings, five unused atomic/volatile effects and five stale gate controls, each at O0, O2 and
with `--taint-all`; a valid volatile-control case returns zero in 24 seeded executions. All 13 new tests
pass; no defect confirmed. This pass excludes `qfree` and ordinary accesses to qubit bytes. It is finite
test evidence, not a proof of the executor.

Checks on main at `40c2800`: `cargo test` 251 passed / 3 ignored; `front/test.sh` 222 passed /
26 ignored, with the C++ header and all 88 LLVM attribute kinds checked; clippy all targets with
warnings denied and formatting clean. All eight device examples also execute: seven deterministic
results match by hand; Grover completes at seed 0 (its distribution is checked by the existing tests).
