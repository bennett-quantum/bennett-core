# HANDOFF

State on 2026-11-18, about 04:05 UTC: **nothing of this work is running.** Resumed by the maintainer on
2026-11-17; see "Resumed on 2026-11-17" below before anything else. The tree implements the model stated in
`docs/DESIGN.md`, "The model": ordinary code; the quantum instructions it issues (`qalloc`, gates, `measure`, `qfree` as the partial trace), naming qubits in
registers or at memory addresses (a derived form, defined in DESIGN by its expansion), with the named fault
`qubit_memory_access` when ordinary code touches a byte that holds qubits; and `apply`, evaluated for each
definite value of its argument registers with one step bound `B` for the whole applied function. Steps 1 to 6
of the order below are done: the independent reference that runs `apply` by the device's construction
(`08d7016`), and applied calls from C (`2464f38`, `4b64089`, `767f853`), with SQLite queries applied to a
qubit-dependent key as their acceptance; an independent testing pass over applied calls (`a0c0bc6`) found no
wrong result and two defects of the front's diagnostics, since fixed. DESIGN's model sections
were read against the written account of the model by an independent reader and corrected where they said
more than the argument carries. Code under a qubit ("Next steps, in order", 4): the two diagnostic defects
are fixed; rule B is done (code without gates under a qubit that writes qubit variables is one `apply`); rule
C is done for a gate loop with an ordinary trip count, not for one deciding on values carried between passes
(a question below); rule A is done for the executor's part and for qubit variables in registers (an
overwrite that cannot be erased keeps the old value, with a warning), not for stores into memory, whose plan
is there, as is the plan for addresses that depend on a qubit. Rule A for stores into memory at a definite
address is done too (`docs/DESIGN.md`, "Memory written under a qubit"), with the programmer's measurement
`bennett_measure_bytes`; the fallback oracle's `store`, `memset`, `nested`, `memcpy` and `loop` fixtures agree
with native C in a harness that reads bytes holding qubits, and in the oracle itself (`f2d0212`), whose adapter
now reads them; the reference that runs `apply` by its construction checks stores by exchange (`f81f2b7`). An
independent testing pass over rules B and C and the first part of
rule A (`03ae832`) left four ignored witnesses in `front/tests/tests/rules_6b_independent.rs`, all fixed
since ("Fixes after the testing pass over rules B and C (2026-11-17)"): a gate loop's guard and its
condition, and one applied function for all results of code under a qubit, results wider than 64 bits and
stores LLVM moves out of the code included. The independent testing pass over rule A at memory (`22bd3bc`)
left five ignored witnesses in `front/tests/tests/held_span_independent.rs`, all fixed since, and a sixth
defect, older, was found and fixed on the way ("Fixes after the testing pass over rule A in memory
(2026-11-18)"). The last independent testing pass, over those fixes and rule A at memory as a whole
(`d11c934`), left three ignored witnesses in `front/tests/tests/held_regions_independent.rs`, all fixed since
("Fixes after the last testing pass over held memory (2026-11-18)"). A randomised differential test against
native C (`18c1973`, `front/tests/tests/qubit_differential.rs`) then found one cause of wrong results in
accepted programs, a value selected where control flow joins after code under a qubit, and three refusals or
messages wider than the rules, all fixed since ("Fixes after the randomised differential test (2026-11-18)").
The test was then widened to the shapes it had left out (`663f78d`), and its 15-minute runs on `main` with two
seeds found no wrong result, no run-time error and no nonzero release in a program marked clean, before and
after the widening; the one shape it found refused where the rules accept it, a store merged at a join, is
accepted since ("Code under a qubit: merged stores and updates in loops (2026-11-18)").

Which commits of code under a qubit have had an independent testing pass:

- tested: `eae3a86`, `d65f156`, `6a0a47a` (by `03ae832`); `ed3cfaa`, `d3a1efe`, `231bea2`, `31246c1` (by
  `22bd3bc`); `7462959` (by `d11c934`); `8668d4a`, `cb4c315` (by `18c1973`, the randomised differential test);
  `49e747f`, `59644f6`, `30b3149` (by the 15-minute runs of `18c1973`'s generator on `30b3149` and of the widened
  generator `663f78d` on `663f78d`, two seeds each);
- not covered by an independent check: the widened generator `663f78d` itself (test-only; its sensitivity was
  shown on the front before `49e747f`) and this HANDOFF commit. The next independent pass starts there.

## Resumed on 2026-11-17

Work was stopped on the morning of 2026-11-17 because the maintainer wanted to understand the computational
model before deciding anything, and was concerned that it might be unphysical. A rigorous account of the model,
with proofs, was written that day. What it showed:

- Ordinary code that issues qubit allocation, gates with controls, measurement and partial trace is the
  standard quantum circuit model with classical control: the first `N` steps of a run, for any `N`, are a finite
  quantum circuit with its measurements deferred to the end.
- `apply @f(args) -> out, fault budget B`, defined for each definite value of the argument registers as running
  `@f` (ordinary code, no quantum instruction) for exactly `B` steps on a private copy of memory and XORing its
  result or fault code into a target it does not read, can be carried out by a device: each ordinary step is
  made reversible by keeping the whole previous state in a fresh record, the result is copied out by XOR, the
  inverse steps run, and the working memory and records return exactly. One `apply` costs `B + 1` steps forward
  (the last marks a run still running as out of budget), the copy and their inverses (`docs/DESIGN.md`,
  "`apply`", with the hypotheses and the cost as an upper bound).
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

The maintainer's further points, about 13:00, 13:15, 13:40 and 15:35 UTC:

- **Memory written under `apply` (about 13:40 UTC).** The rule put to the maintainer: "memory writes made by code
  run under `apply` do not survive it; only the returned value does." The answer: "ok I accept rule 1 in the
  following sense: sqlite. I want to be able to apply sqlite. I understood that to be entirely possible by
  ensuring all memory allocs are *also* simulated and applied". The runtime already has the property: musl's
  allocator, the file model, the clock and the entropy image are ordinary device code over device memory, so under
  `apply` they run on its private memory and are undone with it.
- **Code under a qubit that writes what later code reads (about 15:35 UTC),** on a design note the advisor put
  to the maintainer: "i agree with the fable note. I like the philosophy of Soeren Wilkening: 'everything is
  quantum: if it is in a classical config, and only classical stuff happens, we can approx it as classical. as
  soon as quantum touches it it becomes quantum' measurement is the antithesis of that philosophy". Decided, in
  plain words:
  - A store written in a quantum function's own code, whose value or whether it runs depends on a qubit, makes
    the bytes it writes hold qubits and keeps the value in superposition there: the value is applied into a
    fresh register and exchanged with the target bytes by gates, and the register is cleared when its old
    content can be recomputed (it was known, is a function of registers that have not changed, or the update is
    invertible). The compiler inserts no measurement anywhere.
  - An overwrite whose old value cannot be erased is accepted: the old value is kept in spare qubits that are
    released by `qfree` (the partial trace), with a warning and the count of qubits kept.
  - Measuring bytes at an address makes them ordinary memory again, holding the result.
  - A loop with an ordinary, definite trip count may keep one small `apply` per pass (the front audit's finding
    of one `apply` per pass is harmless as witnessed). Code without gates whose control flow depends on a qubit
    is one `apply` with one budget, whether or not it assigns to qubit variables. The front never creates a
    loop, counter or bound that is not in the C.
  - A loop containing gates keeps a trip count that is an ordinary value written in the C; its ordinary part is
    one applied function returning a table of control bits for the gates, cleared afterwards by the same
    `apply`. No unrolling.

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
   `stackrestore`, `retaddr`, `setjmp`, `longjmp`, `exit`) and qubits in memory by their expansion. An
   independent reader (Muse) compared DESIGN's model sections with the written account of the model and flagged
   21 sentences as unsupported or imprecise; each was checked against that account, and DESIGN now says only
   what it and the code support (`5a4c4a2`): the construction keeps the whole previous state per step, not only
   what a step overwrote; `B` steps and one final step that marks a run still running as out of budget; the
   hypotheses in full; the cost as an upper bound; mixtures after a partial trace; the simulator's `f64`
   tolerance as it is.
5. Independent checks: an audit of the front against the model (done: three wrong translations fixed, "Known
   weak spots"; two findings on loops go to step 6), a testing pass over steps 1 and 2 (done, `214fa8f`: 13
   tests, no defect), a testing pass over step 3 (done, `33ebafa`: 16 tests, no defect; both in "Independent
   checks of apply, removal, front fixes and qfree"), and a small reference that runs `apply` literally by the
   device's construction (done, `08d7016`: 10 tests, agrees with the executor, same section).
6. Done (`2464f38`, `4b64089`, `767f853`; "Applied calls (2026-11-17)" below): the route that replaces the
   front's cancelled fallback: a call whose arguments depend on a qubit, to an ordinary function (no quantum
   operation, directly or through anything it calls), is applied whole with one `budget`, whatever memory it writes, allocates or frees and whatever loops
   and calls it contains; its writes are undone when it ends and only its result survives (a console write or
   `exit` inside is the named fault). Acceptance target: `apply` of a function running a SQLite query whose key
   depends on a small qubit register, on a database set up by ordinary code before the `apply`, each argument
   value compared with native SQLite: passed. Writes that must survive: "Next steps, in order", 4 (decided at
   about 15:35 UTC).

Debts opened by step 2 (removing `mark`, `rewind`, machine states in superposition and the call form):

- **A second implementation of the device language.** The independent reference interpreter modelled the
  removed machinery and went with it; the C rungs 1 and 2 (`experiments/northstar-ladder/`), the musl entry-frame
  edges and several executor tests lost it as a second executor and keep their native or hand-worked
  expectations. Route: an independent reference of the current language, grown from the reference that runs
  `apply` by its construction (step 5, `08d7016`; today a small ordinary subset: integer operations, byte loads
  and stores, fixed call frames, jumps and returns), instruction by instruction with small independent cases.
- **The fallback oracle's self-tests on a fallback-shaped `store`** (`front/tests/tests/fallback_oracle.rs`, module
  `seeded`) built the program with a call reading a register, which ordinary code may no longer do; they were
  removed, and come back with rules B, C and A ("Next steps, in order", 4), on a program the model expresses;
  applied calls (step 6) satisfy none of the fallback oracle's waiting acceptance tests.
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
| `2464f38`, `4b64089`, `767f853` | Applied calls: a call with qubit-dependent arguments to an ordinary function is applied whole with one budget, whatever memory it writes; SQLite queries with a key that depends on a three-qubit register, from C, agree with native SQLite ("Applied calls (2026-11-17)") |

Check it: `cargo test` (279 passed; 3 ignored, each naming its reason: heavy reference, generated or
hash-sample runs with their commands, witnesses of recorded defects and of questions awaiting the maintainer;
about 10 s, no LLVM needed) and `front/test.sh` (239 passed; 29 ignored by default, each naming its reason: the
fallback acceptance tests, waiting for rules B, C and A, the SQLite acceptance of applied calls (needs the
SQLite sources), two witnesses of the applied-call diagnostics' defects, the C North Star ladder runs and other explicit experiments,
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
3. **Independent checks are done** (step 5 of "Resumed on 2026-11-17"): the reference that runs `apply` by the
   device's construction (`08d7016`; "Independent checks of apply, removal, front fixes and qfree") is the start
   of a second implementation of the device language (the debt above).
4. **Applied calls are done** (step 6 of "Resumed on 2026-11-17"; "Applied calls (2026-11-17)" below): a call
   whose arguments depend on a qubit, to an ordinary function, is applied whole with one `budget`, whatever
   memory it writes and whatever loops and calls it contains; SQLite queries with a qubit-dependent key agree
   with native SQLite. **Next, code under a qubit that writes what later code reads** (the maintainer's decision
   of about 15:35 UTC, "Resumed on 2026-11-17"), by an Opus implementer, the plan first rewritten to follow that
   decision, then each a landable step with its tests and documents true in the same step, in this order:
   0. **Done:** the two defects of the diagnostics found by the testing pass over applied calls ("Applied calls
      (2026-11-17)", the end).
   1. **Done: rule B** (`docs/DESIGN.md`, "Code under a qubit"; `front/src/RegionResults.h`). Code without
      gates, written inline, whose control flow depends on a qubit and which writes qubit variables is one
      applied function with one budget for all of it (one `apply` for one result, a pair with CNOTs between them
      for several, one pair for each 64-bit register when the results are wider, all from the same applied
      function), when each variable it writes has a known value before it or is only XORed into; a store LLVM
      moves out of the code to where it ends counts as the code's write. Two nested
      loops whose trip counts depend on a three-qubit register agree with native C for all eight values at O0,
      O2 and with `--taint-all`; the smallest budget is 151 steps and 150 faults the longest runs only; the
      control program keeps no loop. A loop with an ordinary trip count keeps one small `apply` per pass, as
      decided. A variable of unknown value that such a loop reads and overwrites is exchanged (a store by
      exchange with the whole code as the stored function) and its fresh register cleared where its old value
      is known or can be computed again ("Code under a qubit: merged stores and updates in loops
      (2026-11-18)"), and where the old value is neither, the register keeps it and is released (the
      retained form, with its warning and count). Not built: a
      qubit-dependent value carried irreversibly around a loop of the control program (`y = g(y, q, i)` with
      an ordinary trip count), which the design note would merge into one `apply` too; it is refused today
      unless the update is an in-place bijection, and the retained form of step 3 will accept it pass by pass,
      keeping one old value per pass.
   2. **Rule C, partly done** (`docs/DESIGN.md`, "Code under a qubit", gate loops; `front/src/Predicate.h`).
      Done: a loop with gates and an ordinary trip count under a qubit condition (the note's case 1, the front
      audit's witness) stays the C's loop in the control program, entered under its ordinary guard (the
      ordinary part of every test on the way into it), its gates controlled by one condition, that the code
      reaches the loop, computed once before the loop and cleared after it by the same `apply`, which runs none
      of the loop's passes; no unrolling. A loop whose number of passes depends on a qubit (case 4) is
      refused, the message saying that the number of passes must be written as an ordinary value and that
      such a loop does not consume its loop variable; a gate on what the condition reads (case 5) is refused
      by name. **Not built: case 3**, a loop whose passes decide on qubit-dependent values carried from pass
      to pass: refused by name with the table route; why it is not built is a question below ("Questions for
      the maintainer"). Routes for case 5 (a body that changes the tested register but never whether the
      condition holds: one control qubit computed once, cleared by computing it again) are in DESIGN.
   3. **Rule A, partly done.** Done: the executor's part (a `measure` at an address measuring all eight bits of
      a byte makes it ordinary memory again, holding the result; `docs/DESIGN.md`, "Qubits in memory"), and the
      retained form for qubit variables in registers (an overwrite whose old value cannot be erased is
      exchanged into a fresh register released at once, with a warning giving the qubits kept per executed
      overwrite; `docs/DESIGN.md`, "Overwrites that keep the old value"): the fallback oracle's `outside_class`
      passes and is un-ignored (its inventory: 9 checked, 18 waiting by name). **Done since: stores into
      memory at a definite address** (`docs/DESIGN.md`, "Memory written under a qubit";
      `front/src/HeldBytes.h`): the bytes the stores may write (a global, memory a pointer argument points to,
      a local; constant offsets, or LLVM's bound on a position inside code applied whole) get a qubit variable
      of their width, at most 8 bytes, for a span: an ordinary load of their old value and an exchange by three
      CNOTs per bit on address-named bits start it (the shadow then holds a known value), every access in it
      goes to the shadow (so rule B, the in-place inverse and the retained form apply), and the same exchange
      ends it before each return and each measurement of the bytes. A local's stack bytes never hold qubits.
      The programmer's measurement is the marker `bennett_measure_bytes(address, bytes)`, which lowers to
      `measure iN [ADDR]` (`docs/MARKERS.md`); ordinary code reading such bytes is refused, asking for it.
      Fallback tests: `store`, `memset`, `nested`, `memcpy` and `loop` are lowered in all three modes and, with
      the driver measuring the cells (`-DMEASURED_CELLS`, the driver's only change), give native C's results
      for every definite input and its exact weights for all ten preparations in
      `front/tests/tests/memory_held.rs`, which reads bytes that hold qubits from their registers. They pass
      in the oracle itself too since `f2d0212`: its adapter decodes the whole image for each basis
      configuration from the snapshot's address table, and its driver measures the four result cells first
      (`front/tests/c/fallback_oracle/README.md`; the inventory checks 24 accepted fixture and mode
      combinations). `user_write` waits by decision (the route of a call that fills memory named at the
      call). Debts, each refused by name: a store into memory in code that also has
      gates (route: predication of whole stores into the shadow, as for qubit variables); pointers that may
      reach the same bytes as a span's object (a second object written under a qubit, an ordinary access
      through another pointer; route: a check of the addresses at run time choosing one shadow or two, not
      built; what removes it today is a static function whose calls pass distinct objects or the same pointer,
      or `restrict`); a call that may read or write the held bytes, a callee that measures them included
      (route: end the span before a call whose only access to the bytes is `bennett_measure_bytes`, not
      built; today the caller measures them first); a second shadow of bytes another write under a qubit still
      holds (an earlier store that does not run before the code under a qubit on every path, or that is in a
      loop; route: one shadow from the earliest write, not built); a `restrict` argument whose address
      escaped, while its bytes hold qubits, against any call or access that may follow escaped memory (route:
      follow which escaped copies a callee may load, not built); more than 8 bytes of
      one object (one shadow per 8 bytes); an access that touches the held bytes only in part; a position that
      is not constant outside code applied whole; an object the front cannot name (a pointer loaded from
      memory: step 4's region); a measurement of part of a local's held bytes. Not refused, documented: where
      LLVM's bound is wider than the C's writes (`memcpy` of 2 or 4 bytes is bounded at 6) the extra bytes hold
      qubits with their old values; the front assumes the bytes are ordinary where a span starts, so a second
      function's store under a qubit into bytes an earlier call left holding qubits is the fault
      `qubit_memory_access` (a question below). Found on the way, older than this work: after a qubit
      variable's value became unknown, `if (r) y += 2` is predicated as an overwrite and takes the retained
      form instead of the in-place inverse (route: invert the predicated store in place when the update is a
      bijection on a condition that does not read y).
   4. **Rule A at an address that depends on a qubit: plan** (not built). The region every possible target lies
      in is taken from the object the address is based on when its extent is known (a global, a fixed-size
      local), else refused with the route "declare the storage" (a marker naming a region: a language choice
      for the maintainer). The access is the exchange of one addressed element: `n ^= R[i]` (`apply`,
      arguments the index and the region, target a fresh `n`), `R[i] ^= n` (for each element `j` of the
      region and each bit, an X on the element's bit controlled by `n`'s bit and by "i == j", the latter as
      X-conjugated literal controls on the index's bits or computed by a decoder that returns its workspace to
      0), `n ^= R[i]` again; `n` then holds the old element and is cleared as at a definite address, else
      retained. The loop over the region's elements is a loop of the control program over the object's size,
      which the C states. Cost: on the order of the region's size times the element's width in gates per
      access, which is also a lower bound for coherent addressed exchange. The index must not lie in the
      region; an applied function cannot read qubit-held bytes, so the region is copied into registers by
      CNOTs around each `apply` that reads it (several registers past 64 bits, or the derived form asked
      about under "Questions for the maintainer").
   Route recorded, derived, not built: **k successive applied calls on memory kept in qubits** can run with
   nothing retained when the argument registers of all the calls are kept and the memory starts in a known
   definite state: the old memory before call j is computed again from the earlier calls' arguments. Cost
   k(k+1)/2 evaluations of the calls when done call by call, k when the sequence is one applied function.
   This is the route for a stateful library whose writes must survive.

   Then one independent testing pass over steps 1 to 3 (rules B and C and the first part of rule A: `03ae832`,
   its four defects fixed since; rule A at memory: `22bd3bc` and `d11c934`, their eight defects fixed since,
   the last three by commits no independent pass has tested), and the reference that runs `apply` by its
   construction extended with the exchange form of rule A (done, `f81f2b7`; `docs/DESIGN.md`, "Tests"): the
   exchange with each way of clearing the spare register on exact amplitudes, a byte at a definite address
   included, and for an old value kept and for a fault inside an applied function every entry of the reduced
   density matrix after the release; one ignored witness, the reference does not model the stack space
   available where an applied call starts (`crates/exec/tests/compute_copy/swap_in.rs`).
   No step inserts a measurement, bounds a loop on its own, or lets ordinary code act on a qubit outside
   `apply`; a choice that would is the maintainer's. The Boolean XOR on a quantum `_Bool` (`y ^= q && !r`, and
   `if (q) if (r) y ^= 1` at O0 and with `--taint-all`) already passes the fallback oracle (`coherent_xor`,
   `coherent_nested`).

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
   changed by this experiment. Larger process budgets remain unmeasured. Since applied calls (step 6 of
   "Resumed on 2026-11-17") the queries are also applied from C, with no hand-written device entry (stage 4).

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

Found correct: the SQL oracle's marked sets, recomputed from the query semantics by host SQLite and by hand, at
every shape and size, and every device Grover amplitude at N=16 to 512 against a separate dense calculation; an
edge-row database (NULLs, duplicates, 64-bit bounds, collation, overflow, all and no rows), all 56 checks
(`experiments/sqlite-on-device/edge/`). The entry frame at every memory size around it, seven signatures, three
data layouts, four option sets, and with musl startup at every stack size near both edges, against the independent
reference interpreter of the time (removed on 2026-11-17) (`crates/exec/tests/entry_frame_boundary.rs`,
`front/tests/tests/entry_frame_musl.rs`). The Lua probe's three stages rerun, five more scripts (error messages,
format corners, garbage collection under pressure, integer/float edges, ordered tables) agreeing and reversing
exactly, a second predicate with three of eight marked, and every Grover amplitude against closed forms
(`experiments/lua-probe/README.md`, "Independent testing pass").

## Questions for the maintainer

Gone through on 2026-11-17 after the maintainer's decisions ("Resumed on 2026-11-17"); none is answered. Withdrawn,
with the machinery they concerned (`mark`, `rewind`, machine states in superposition, the call with a fault value;
git has their text): Choices made in machine-sites (2026-11-17), not yet asked; Independent reference snapshot
storage (2026-11-17); From the design seam and its council (2026-11-16, night; see "Design audition of
2026-11-16"); A rewind naming a mark that an outer rewind un-ran; Which fault a rewind gives when running again
would pass the budget; Superseded (2026-11-17): the leftover is now taken back; Time and the run's budget; How the
budget is required; The call with a fault value; Reference-only clock/diagnostic boundaries; A budgeted callee's
padded clock and its caller's mark; A mark's budget once its frame is left.

- **Which bytes hold qubits, for the fault `qubit_memory_access` (2026-11-17, step 3 of the work). ANSWERED
  at about 15:35 UTC:** measuring bytes at an address makes them ordinary memory again, holding the result
  ("i agree with the fable note. I like the philosophy of Soeren Wilkening: 'everything is quantum: if it is in a
  classical config, and only classical stuff happens, we can approx it as classical. as soon as quantum touches it
  it becomes quantum' measurement is the antithesis of that philosophy"; "Resumed on 2026-11-17"). **Built** (step 3 of
  "Next steps, in order", 4): a `measure` at an address that measures all eight bits of a byte makes it ordinary
  memory holding the result; a byte measured in part keeps holding qubits (`docs/DESIGN.md`, "Qubits in
  memory"). Still open: a release of qubits at an address without measuring, as `qfree` is for a register (a
  new instruction, so the maintainer's to decide).
- **Ordinary code that needs a definite value from bytes holding qubits (2026-11-17, taken by the advisor
  with the decision of about 15:35 UTC).** Where ordinary code needs such a value (console output, a jump in
  control code, the exit code), the front refuses and asks for a measurement the programmer writes; it does not
  insert one. Cheap to reverse; the alternative would be a measurement the compiler inserts, which the decision
  rules out for stores.
- **Where a span of memory written under a qubit starts, when an earlier call may have left the bytes holding
  qubits (2026-11-17, rule A at memory; taken, cheap to reverse).** The front reads the bytes' old value with an
  ordinary load where the span starts, so the shadow's value is known (the clean form). It cannot see across
  calls whether the bytes still hold qubits from an earlier call; if they do, that load is the named fault
  `qubit_memory_access`. Options: (a) as taken; (b) without the load, the exchange alone (correct whatever the
  bytes hold, but the old value is then unknown: overwrites keep it, with the warning, and rule B refuses loops
  that overwrite); (c) the control program keeps, in ordinary memory, a flag per object saying whether its bytes
  hold qubits, and the front writes both forms (no new instruction). Recommendation: (a) now, (c) when a program
  needs it.
- **A function the host may call, with pointer arguments, while memory holds qubits (2026-11-18, rule A at
  memory; taken, cheap to reverse).** The front cannot see the calls the host makes to a function with external
  linkage, so it treats such a function as callable with any pointers: two of its pointer arguments may name the
  same bytes, and either may name a global it reads. Where that could reach bytes holding qubits, the call, the
  access or the second object is refused by name with the route (`docs/DESIGN.md`, "Code under a qubit").
  Alternatives: (a) as taken; (b) treat the entry functions the tests and the host name as the only external
  callers, with the arguments they pass; (c) a check at run time of the addresses, choosing one shadow or two.
  Recommendation: (a) now, (c) when a program needs it.
- **The table of control bits for a gate loop whose passes decide on qubit-dependent values carried between
  passes (2026-11-17, rule C, not built; nothing taken).** `t = x; for (i = 0; i < n; i++) { if (t & 1)
  h(y); t = step(t); }` with `n` an ordinary run-time value. The decided form is one applied function
  returning a table of control bits for all passes, computed before the loop and cleared after it. Two
  things the C does not give: the table's width (one bit per pass and gate site, so as wide as `n`, a
  run-time value), and a way for the control program to name the table's bit for pass `i` (a register bit is
  named by a constant). Options, none of which adds a bound per loop: (a) build the table only when the trip
  count is a constant in the C of at most 64 passes times gate sites, the control program choosing the bit
  of pass `i` by comparing `i` with each constant index (code in proportion to the table, no new instruction);
  (b) for any `n`, one small `apply` per pass computing that pass's decision from `x` and `i` (it reruns the
  passes before it inside the applied function: about `n²/2` passes of work in all, each `apply` with its
  budget, allowed by the decision on loops with an ordinary trip count); (c) the table in qubit-held memory
  bytes indexed by an ordinary address, after rule A at addresses. Recommendation: (a) first (exact, cheap,
  covers loops LLVM did not unroll), then (b) for run-time counts. Today the loop is refused by name.
- **Results of code under a qubit wider than one register (2026-11-17, rule B; taken, cheap to reverse).** A
  register holds at most 64 qubits, and one `apply` writes one register. Taken: the code stays one applied
  function, which takes the number of a 64-bit result word as an ordinary argument and computes every word
  before choosing the one asked for (so each run takes the same steps); each word is one `apply` into its
  variable or one pair around a fresh register, all with the same budget. Cost: two runs of the whole code per
  register instead of two in all. Options: (a) as taken; (b) refuse results wider than 64 bits by name; (c) a
  register or an `apply` target wider than 64 qubits, a change of the device language. Recommendation: (a)
  until a program's cost shows otherwise, then (c).
- **Applied calls (2026-11-17, step 6; each taken, cheap to reverse).** A call with qubit-dependent arguments
  whose result is unused, to a function that may write memory: taken, refused by name (applied, it would do
  nothing; its writes are its purpose). Recommendation: keep. Two such calls in one computation when one may
  write memory: taken, refused by name (in one applied function the second would see the first's writes, while
  under the rule each starts from memory as it was). Route: one register per call, computed before and cleared
  after the computation that uses it. Recommendation: build it when a program needs it. The warning's reach:
  taken as written in "Applied calls (2026-11-17)"; no warning proves nothing. Recommendation: keep, extend if a
  real program misses it.
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
| A store into memory under qubit control outside a call; a value needed after it changed; qubit variables in structs, heap objects or passed to functions; gates in a loop under qubit control whose trip count depends on a qubit or whose passes decide on qubit-dependent values carried between them | A call with qubit-dependent arguments, whatever it writes, is applied whole since step 6 ("Applied calls (2026-11-17)"). Code without gates under a qubit that writes qubit variables of known value or XORs into them is one `apply` since rule B. Gates in a loop with an ordinary trip count are kept in the loop since rule C. For the rest, the decision of about 15:35 UTC ("Next steps, in order", 4): a gate loop deciding on values carried between passes (the table of control bits); stores that keep their value in qubits at the bytes written (an overwrite of a qubit variable that cannot be erased keeps its old value in qubits released by `qfree` since rule A's first part) |
| A qubit-dependent value used by ordinary code or printed; measurement under control | Channels |
| A qubit-dependent call, to a function that may write memory, whose result is unused; two such calls in one computation when one may write memory; quantum operations in `main` of a program linked with musl | "Applied calls (2026-11-17)": nothing to remove for the first (applied, it would do nothing); one register per call for the second; classifying control functions through indirect calls, or a startup that calls `main` directly, for the third |
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

- **Two findings of the front audit of 2026-11-17 on loops** (decided at about 15:35 UTC; step 4 of "Next
  steps"; code under a qubit without gates is one `apply` since rule B, so a loop whose trip count depends on a
  qubit never runs one `apply` per pass). (1)
  The front writes one `apply` per update of a qubit variable, so `for (i = 0; i < n; i++) y ^= f(q, i) & 1;`
  is `n` applications, each with its own budget and fault register, where one `apply` of the whole loop's XOR
  with one budget would do; a loop inside a function that only reads memory, called with arguments that depend
  on a qubit, already runs under one `apply` and one budget. With an ordinary, definite trip count this is
  harmless and may stay. (2) Gates in a loop under a qubit condition: done for an ordinary trip count since
  rule C (`if (q) for (i = 0; i < n; i++) bennett_x(&y);` is one controlled X per pass); a loop whose passes
  decide on qubit-dependent values carried between passes is still refused (the table route; question below).

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
- Models in use for this work since 2026-11-17: implementers Claude Opus 5.5; independent tests, reviews and
  references gpt-6.2-sol (gpt-6.1-sol did the passes of 2026-11-17 until the evening); Muse as a reader.

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

## Independent checks of apply, removal, front fixes and qfree (2026-11-17)

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

The second pass (`33ebafa`) tests `qfree`, the fault `qubit_memory_access` and the simulator's stops: 16 tests,
expectations worked by hand. Reduced density matrices (computed by the test from the snapshot, not by the
executor) after releasing parts of Bell and phased GHZ states held in separate registers, after controlled
rotations down to a released weight of about 5e-31, after repeated releases and around an `apply` target agree
with hand calculation (`release_independent.rs`). Every integer width from 1 to 128 is loaded and stored at
every byte overlapping one that holds qubits and its neighbours, 8,064 cases (6,528 faults, each leaving the
whole state unchanged); faults inside `apply` per argument address XOR code 17 into a dirty fault register;
all 32 jump-record bytes, copying loops and a measured stack byte after its frame returned are covered
(`qubit_access_independent.rs`). The configuration limit and the stop at an `apply` faulting for some argument
values leave the state as before the step and let no later effect run; gates and measurements at addresses
inside an applied function give `quantum_in_region` (`simulator_stops_independent.rs`). No defect confirmed.
Checks on that commit: `cargo test` 269 passed / 3 ignored; `front/test.sh` 222 passed / 26 ignored; clippy
and formatting clean. Finite tests in `f64`, not a proof.

The reference that runs `apply` by the device's construction (`08d7016`, gpt-6.1-sol;
`crates/exec/tests/compute_copy_reference/`, `compute_copy.rs`) is a small interpreter of its own over the
public IR data, with its own arithmetic, byte memory, calls and fault codes, sharing no stepping code with the
executor. Its state vector holds explicit argument, `out`, `fault` and machine registers and one full record of
the previous machine state per scheduled step: each step XORs its update into its record and exchanges record
and machine, exactly `B` steps run forward (idle after a return or fault), the result or fault code is XORed
into the targets, and every inverse step runs; every machine and record bit must return exactly. Ten tests,
nine small functions, 83 function and budget comparisons over four argument values each: 5,312 configuration
images with exact complex coefficients (both signs and imaginary phases) agree with the executor within 1e-14
per component, and 332 separate evaluations for single argument values agree. Cases: wrapping arithmetic,
private byte writes and reads, a call, a loop with an argument-dependent trip count, two nested loops with
trip counts `x` and `x + 1`, division by zero after a private write, an out-of-range read, `effect` and `exit`
faults, and `B` as a run-time argument, one bound for the whole function (at `B = 5` the simple loop returns
for `x = 0` and runs out of budget for 1, 2 and 3). Four deliberately broken variants of the helper (inverse
exchange left out, one output bit wrong, an argument phase removed, the budget code missing) each fail their
test. It covers only its small instruction subset (no dynamic stack allocation, indirect calls, non-local
jumps or qubits in memory); extending it is the route to a second implementation. Finite test evidence, not a
proof.

## Applied calls (2026-11-17)

Step 6 of "Resumed on 2026-11-17", landed as `2464f38` (the rule, its refusals and the warning), `4b64089`
(the write summary computed once per module: the front on the SQLite module from 208.6 s to 40.2 s, output
byte-identical) and `767f853` (the SQLite acceptance). Documents: `docs/DESIGN.md`, "Applied calls";
`docs/MARKERS.md`, "Calls with qubit-dependent arguments".

- **The rule.** A call whose arguments depend on a qubit, to an ordinary function of the program (no quantum
  operation, directly or through anything it calls), is applied whole with one budget, whatever memory it
  writes, allocates or frees and whatever loops and calls it contains; its writes are undone when it ends and
  only its result survives. A console write or `exit` inside is `effect_in_region` or `exit_in_region` for the
  argument values that reach it (a musl `printf` of a line too; output left in musl's buffer is memory and is
  undone). Smallest budget for all eight values of the small fixture: 34,593 steps; at 34,592 only the value 2
  faults.
- **The warning.** Natively the writes would remain. The front warns, and still lowers the program, when the
  called function, or a function it calls directly, writes a global variable or memory passed to it by pointer
  that later code in the caller reads, directly or through a function it calls directly. Writes through other
  pointers and reads through indirect calls are not followed, so no warning proves nothing.
- **The rule is about calls.** With the project's clang flags (`-O0 -Xclang -disable-O0-optnone`) every call
  stays a call; IR compiled otherwise may arrive with a callee inlined, whose stores are then refused by name.
- **Acceptance: SQLite from C** (`experiments/sqlite-on-device`, stage 4; `front/tests/tests/sqlite_applied.rs`,
  ignored by default because it needs the SQLite sources):
  `SQLITE_DIR=<amalgamation directory> front/test.sh --test sqlite_applied -- --ignored --nocapture`.
  Each of the SQL oracle's eight queries is applied to a key that depends on a three-qubit register, on a
  database set up by ordinary code: right after each `apply`, exactly eight configurations of amplitude
  1/sqrt(8), each with the native result for its row and a zero fault register, and the whole classical
  memory, database included, unchanged; strict mode, budget 1,000,000 steps per `apply`, no warning. 84 s on
  the shared laptop with the musl archive and SQLite objects cached (builds 54 s, a cold build 151 s; the front
  26 s; each query 0.2 s). The Lua probe's C marker, refused before, now lowers and runs
  (`experiments/lua-probe/README.md`).
- **Debts.** A program linked with musl cannot hold quantum operations in `main`: musl's startup passes it by
  pointer, and the front's dispatch of indirect calls makes many musl functions call it, so the first refusal
  names the address of a musl function that "performs quantum operations"; the fixtures use an entry of their
  own and keep `main` ordinary. Route: classify control functions through indirect calls, or a startup that
  calls `main` directly. Nothing in the front checks that calls arrive as calls; proposed route: name an
  inlined callee from its debug location when a store it made is refused or kept in qubits, and mark calls in
  control functions `noinline` before the front's own optimiser. `cargo clippy -p bennett-front-tests
  --all-targets` reports two `unwrap` uses in `front/tests/tests/complex.rs` (outside the four checks). The
  fallback oracle's seven waiting acceptance tests stayed ignored then: each needed a write that survives or
  an erasure, rules B, C and A; since `f2d0212` only `user_write` waits.

**Independent testing pass** (`a0c0bc6`, gpt-6.2-sol; `front/tests/tests/applied_calls_independent.rs`,
fixtures `front/tests/c/applied/independent.c`, `rollback.c`): nine tests, every memory byte, the stack pointer,
the console tape and the register table compared around the `apply`. A recursive call with a variable-length
array in each frame, writing through a pointer to the caller's local and a global, and an internal `setjmp`
with a `longjmp` across recursive frames, agree with native for all eight argument values at O0, O2 and O0 with
`--taint-all`, the target starting nonzero. Nested loops in the applied call share one budget: the callee
alone takes 21, 58, 119, 204, 313, 446, 603, 784 steps for inputs 0 to 7, and with budget 313 exactly 5, 6 and
7 fault. musl's `realloc` and `free` of a caller's object and file writes, seeks and reads through a duplicated
descriptor are undone byte for byte, allocator and file model included, and later ordinary code sees the
original file and heap. Console output and `exit` give their faults per value with an empty tape; a load and a
store at a byte holding qubits give `qubit_memory_access` for the values that reach them. Three deliberately
wrong expectations each fail. The SQLite acceptance, run again on its own build: 64 comparisons pass. No wrong
result. Two defects of the diagnostics, each a witness in that file, fixed since (step 0 of "Next steps, in
order", 4):

- **The warning misses a caller-local variable read by a direct callee** (`r ^= put(v, &local)` then
  `get(&local)`: no warning). The write side keeps the local allocation (`front/src/Applied.cpp`, around line
  140); the direct callee's read keeps only globals and formal parameters (around lines 24 and 165).
- **`--taint-all` refuses a writing call combined with a pure call on an ordinary argument** (`r ^= bump(v) +
  score(n)`, lowered without the option to the native 46 and 47): the slice admits the pure call
  (`front/src/Control.cpp`, around line 493) and the count of qubit-dependent calls then includes it (around
  line 519), giving the refusal of two calls in one computation. Fixed: only calls that depend on a qubit
  count; and an ordinary call `--taint-all` moves into an applied function now runs before anything that depends
  on a qubit there. Without the second part the fix would have exposed a wrong result: in
  `r ^= bump(v) * 2 - peek(n)` the ordinary `peek` read the private copy after `bump` wrote it (14 instead of 15
  for v = 1; regression test `an_ordinary_reader_in_a_slice_does_not_see_the_applied_calls_writes`). The
  warning now also follows a later direct call given a pointer to the caller's local variable.

## Fixes after the testing pass over rules B and C (2026-11-17)

The independent testing pass over rules B and C and the first part of rule A (`03ae832`, gpt-6.2-sol) left four
ignored witnesses in `front/tests/tests/rules_6b_independent.rs`, each reproduced and each failing on the code
before these fixes; all four run now. Documents: `docs/DESIGN.md`, "Code under a qubit".

- **A gate loop nested in another lost its zero-trip guard** (`entry(n = 0, m = 0)` faulted `budget` instead of
  returning). Cause: the guard was read from the loop's predecessor only. With a nested loop LLVM hoists the
  inner loop's guard into a preheader of the outer loop's own, which ends in an unconditional branch, and merges
  the outer loop's guard `n != 0` into the branch on the qubit one block above; the front found no test and
  entered the loop always, so with `n = 0` it ran through the counter's whole cycle. Fix
  (`front/src/Predicate.cpp`, `entryGuard`): the guard is the ordinary part of every conditional branch one of
  whose edges every path to the loop takes, from the loop's predecessor up to the branch that enters the code
  under the qubit.
- **The condition of a gate in a loop replayed the loop's ordinary passes** (`if (q) for (i < n) if (i ==
  site) x(y)` with budget 32 faulted from `n = 6, site = 5`). Cause: the condition was "the code reaches this
  gate", computed by a copy of the code that runs until its first visit to the gate, so through `site` passes of
  the loop. Nothing in such a loop depends on a qubit (anything that does is refused), so which gates act in
  which pass is decided by the loop's own ordinary code, which the control program runs anyway. Fix
  (`moveLoop`): one condition for the whole loop, "the code reaches the loop's header", whose copy returns
  before the first pass; its steps no longer grow with the trip count or the gate's position. One bound per
  `apply` as before; no bound, loop or counter added.
- **A known variable whose stores LLVM moved out of the loop got an applied copy of its own** (`b = 3 * b + d`
  in the loop that XORs into `a`: two applied copies of the loop). Cause: scalar promotion leaves `b` in a
  register through the loop and one store after it, in the block where the code ends; the front collected only
  stores inside the code, so the value left it and was computed again by the `apply` that stores it. Fix
  (`front/src/RegionResults.cpp`, `movedStores`): such a store is a write of the code when the block is entered
  from the code only and begins with it, the variable is not touched in the code and its value before is known;
  the value is a field of the same result, and the store goes. The acceptance fixture of rule B had the same
  defect (its counter `c` was computed by a separate copy): its smallest budget is now 151 steps, not 128, all
  three results under one pair of `apply`s.
- **Results wider than 64 bits were split into two applied functions.** Cause: the results were packed into
  64-bit words and the code copied once per word. Fix: one applied function, the word's number an ordinary
  argument (the question "Results of code under a qubit wider than one register" above).

Regression tests besides the four witnesses: `front/tests/tests/gate_loops.rs` (nested gate loops at every
pair of trip counts 0 to 2, and a gate at pass 199 of 200 under a budget of 16 steps, in O0, O2 and
`--taint-all`) and `front/tests/tests/region_whole.rs` (one pair for the fixture's three results; 64 + 32 + 16 + 8
bits of results from one loop, one applied function, values checked in all three modes); each fails on the
code before. Debt, in DESIGN: a value the code computes that later code uses other than by such a store (a plain
local assigned in the loop and used in arithmetic after it) is still computed again by a copy of the code in
the `apply` of its use; correct, one more run of the code.

## Fixes after the testing pass over rule A in memory (2026-11-18)

The independent testing pass over rule A at memory (`22bd3bc`) left five ignored witnesses in
`front/tests/tests/held_span_independent.rs`; each was reproduced, each fails on the code before these fixes, and
each runs now at O0, O2 and with `--taint-all`. Documents: `docs/DESIGN.md`, "From C to a device program",
"Overwrites that keep the old value" and "Memory written under a qubit"; `front/src/HeldBytes.h`,
`front/src/Aliasing.h`.

- **A measured byte was still refused when the span had more bytes.** Cause: the span's state was one value for
  all its bytes, and a measurement of part of them set it to "holds qubits". Fix (`front/src/HeldBytes.cpp`): after
  the span ends the state names the bytes that still hold qubits; a measurement removes its bytes, and ordinary
  code may touch the measured ones. The witness ran its program with the helper that forbids a register released
  not 0, which this program releases (q keeps the value the measurement found): it uses the helper that reports it.
- **A span ended by a measurement on one path and by a return on another was refused.** Cause: where the paths
  join, "the shadow holds the bytes" met "the span has ended", a conflict that moving the start earlier cannot
  settle. Fix: outside code under a qubit, the span ends on the edges from the paths where the shadow still holds
  the bytes (the same exchange as before a return).
- **A callee reading a held global variable was accepted; the run faulted `qubit_memory_access`.** Cause: only a
  call's pointer arguments were checked. Fix (`front/src/Aliasing.cpp`): a call reaches what its arguments point to
  and what its callee and the functions it calls access; one that may reach the held bytes is refused by name,
  asking for the measurement.
- **Two pointer arguments given the same byte opened two spans over it; the run faulted.** Cause: one shadow per
  LLVM object, and objects were assumed never to share bytes. Fix: what each pointer may point to, from the calls of
  its function in the module (any address for a function the host may call). Two objects written under a qubit that
  may share bytes, or an access through another pointer that may reach held bytes, are refused by name with the
  route; a static function whose calls pass the same pointer for two arguments has them made one object (C's last
  write results), distinct objects and `restrict` arguments get one shadow each. The witness's function has external
  linkage, so the host may call it with two different pointers or the same one: it now expects the refusal, and the
  merged, distinct and `restrict` cases run.
- **An overwrite whose old value is a function of an unchanged register took the retained form** (8 qubits,
  interference lost). Cause: the dataflow's facts were "known ordinary value" or "unknown", so a variable holding a
  value computed from a qubit had no old value the front could use. Fix (`front/src/Taint.h`, `Control.cpp`): a third
  fact, the value stored whole; the overwrite applies that computation again into the variable (clearing it) when
  every register it reads is unchanged and its calls see the same memory, then the new value; a read of the old
  value in the new one (`if (r) a = 7`) is that computation, and with `--taint-all` a load that read a known value is
  given it. Tests: the witness (P(q=1) = 0 after the final H, nothing kept), the controlled overwrite (exact
  amplitudes), and one whose source register changed in between, which keeps the retained form and its 8 qubits.
- **Found on the way, older: a region writing two objects drove wrong bits** (`if (q) { a = 5; b = 7; }` from
  3 and 3 gave 7 and 7: a wrong ordinary result). Cause: the result word's fields drive only the bits a variable may
  hold, and that support left out the old value the shadow takes at the exchange and put a store into part of a
  shadow at bit 0 instead of at its bytes. Fix (`front/src/Taint.cpp`, `computeSupport`). Test:
  `two_objects_written_in_one_region_get_every_bit_of_their_change`.


## Fixes after the last testing pass over held memory (2026-11-18)

The independent testing pass over the fixes above and rule A at memory as a whole (`d11c934`) left three ignored
witnesses in `front/tests/tests/held_regions_independent.rs`; each was reproduced, each fails on the code before
these fixes, and each passes now at O0, O2 and with `--taint-all` (the first runs with C's value, the other two
are refused by name, which their witnesses accept). Documents: `docs/DESIGN.md`, "Memory written under a qubit";
`docs/MARKERS.md`; `front/src/HeldBytes.h`, `front/src/Aliasing.h`.

- **An earlier straight store and a later region writing the same byte gave the earlier value** (C: `a = 5` then
  `if (r) { a = 7; b = 9; }` gives 7; the device gave 5). Cause: the region's writes were held first, and the
  straight store, held afterwards, opened a second shadow of the same byte whose span took the first shadow's
  start as an ordinary access; both shadows were exchanged into the byte at the end. Fix
  (`front/src/Outline.cpp`, `HeldBytes.cpp`): a store of a qubit-dependent value that runs before the first code
  under a qubit on every path and is in no loop (no qubit can control it) is held first, so the region's write is
  an access to the same shadow; and a span that meets another shadow's exchange of bytes it may hold, while they
  hold qubits, is refused by name with the route (one shadow from the earliest write). Tests in
  `memory_held.rs`: three shapes of earlier store and later region (C's values, nothing kept), and the order the
  front cannot give one shadow (an earlier store under an ordinary condition), refused; on the old code it gave
  wrong values.
- **A callee measuring a held global read the cleared byte** (0 for 5). Cause: the summary of what a function
  touches left out every marker call, `bennett_measure_bytes` included, which reads the memory it measures. Fix
  (`front/src/Aliasing.cpp`): that marker reaches the objects its address may point into, so the call is refused by
  name, asking for the measurement in the caller. Test: a measuring callee two calls down, refused (accepted and
  wrong on the old code); callees measuring another global, or the bytes after the caller measured them, still run.
- **A callee reading a saved copy of a `restrict` argument faulted `qubit_memory_access`.** Cause: a `restrict`
  argument was taken never to share bytes with a pointer the front cannot follow, even when a copy of its address
  escaped; a pointer loaded from where that copy was stored is based on the argument, and C allows it to reach the
  bytes. Fix: such a pointer may reach the argument's bytes exactly when its address escapes. Test: the copy read
  back in the same function, refused (accepted and faulting on the old code).

Debts found by this work, not fixed (diagnostics and refusals that may be over-cautious): the refusal of a
measuring callee reads "a call that may read or write memory of ..." and does not say that the callee measures
them; the refusals of a second shadow, of a measuring callee and of an escaped `restrict` copy are wider than
what is unsafe (the routes above, under "Next steps", step 4, item 3); the full front suite prints an unused
import warning in `front/tests/tests/thesis.rs`.

## Fixes after the randomised differential test (2026-11-18)

The randomised differential test `front/tests/tests/qubit_differential.rs` (`18c1973`: seeded short C programs in
the accepted class, compared with native C for every definite value of the register, at O0, O2 and with
`--taint-all`) found 21 wrong results in accepted programs in a 15-minute run on `cb4c315`, all of one cause, and
left five ignored witnesses. Each was reproduced on the code before these fixes and each runs now in all three
modes. Documents: `docs/DESIGN.md`, "Code under a qubit" and "Applied calls".

- **A value selected where control flow joins was lost when code under a qubit was applied whole** (`if (q & 2)
  a = 2; if (q & 1) a += q;` gave 3 for `q = 1`, C 1; `if (!q) { a = 2; b += 3; } if (q) a = 4;` gave 2 for
  `q = 1`, C 4). Cause, in plain words: after the code's writes become one applied function, the front deletes
  what is left of the code unless a value it computes is used later; it looked only at values computed by
  instructions inside the code. A join where the code ends (an LLVM phi of the block after it) that selects by
  the edge the code left on, of constants or of values from before the code, is decided by the path the code
  took, but no instruction of the code computes it; the code was deleted and the join kept one of its incoming
  values for every input. Fix (`front/src/RegionResults.cpp`, `RegionWhole::removeStores`): such a join keeps
  the code's control flow, which is then outlined as any code whose value later code uses, the join a result
  of that function; a store LLVM moved to the join is then an ordinary store of that value. Tests: the two
  witnesses, now in all three modes, and `join_selects_the_value_of_the_path_taken` (constants, values from
  before the code, a merged store, a three-way join); three of its four shapes give wrong values on the old code.
- **A helper whose only memory is its own stack was refused as reading memory that may change, and as a second
  call that may write memory.** Cause: the checks asked whether the callee accesses memory at all, wider than
  the rule (the memory the call reads must be unchanged; two calls in one computation must not see each other's
  writes). Fix (`front/src/Aliasing.cpp`, `reachesOutsideFrames`; `front/src/Control.cpp`): a call reads and
  writes only memory its callees may reach outside their own frames; a local variable of a callee, written
  directly or by a helper it calls through a pointer, is none. A callee local whose address escapes still counts.
  Tests: the two witnesses, and `nested_private_frames_and_an_escaped_local` (the nested case is refused on the
  old code).
- **A call whose unused result LLVM removed got the generic message** ("used by ordinary code (measure it
  first)"). Cause: a call returning nothing has no value that depends on a qubit, so it reached the check for
  ordinary uses of its arguments. Fix (`front/src/Control.cpp`): such a call to an ordinary function that passes
  nothing by address gets the message for a call whose result is not used (one with a pointer argument may
  return its result through memory, `front/src/Abi.h`, and keeps the message it had).

Commits: `49e747f` (the join), `59644f6` (the helper's frame and the message).

Capped run of the long test after the fixes (`BENNETT_RANDOM_CASES=1600`, seed `0x87196caa251d0f43`, which covers
indices 697, 885, 905, 967, 1288 and 1550 of the 21): generated 1,600, accepted 1,196, declined 404, compared
8,084, wrong 0 (with the first fix alone: accepted 1,084, compared 7,296, wrong 0). The full 15-minute run
follows on `main`.

Debts: a join the code leaves for later code is computed again by a copy of the code in the `apply` that uses it
(the existing debt in DESIGN, "Code under a qubit", whose route covers it); the refusal of an unused call whose
callee writes only its own frame stays (applied, it would do nothing, as natively nothing observes those writes;
accepting it is a widening, not taken); a call that does read memory outside its frames is still refused when
any write may come between it and its use, whether or not the write can reach what it reads (route: compare the
call's reach (`Aliasing`) with each intervening write's). In the capped run 8 programs were still refused with
"reads memory that may change before its result is used"; in those looked at the call is an outlined region of
code under a qubit that reads global variables, not a helper (older, not investigated).

## Randomised differential runs (2026-11-18)

The test `front/tests/tests/qubit_differential.rs` (generator in `qubit_differential/generator.rs`) writes small
seeded C programs in the class the front accepts and compares the device's result with native C for every
definite value of the prepared register and every result packet, at O0, O2 and with `--taint-all`. `663f78d`
added recipes for gate loops under a qubit-dependent condition with ordinary trip counts, calls with
qubit-dependent arguments to a function that writes memory the caller reads later (the front's warning
checked), calls that write memory under a qubit-dependent condition (the named refusal checked), results of
120 and 152 bits in four objects, an object written in two regions or before and inside one, the overwrite that
keeps its old value (its warning and exactly one nonzero release of that many qubits checked), and bytes measured
by the program and then used as ordinary memory. It still leaves out independent quantum inputs, phases and
interference, large loop counts, uncertain aliasing, variable offsets, memory copies and calls to outside code.
The default run takes a few seconds; the 15-minute run is ignored:

```sh
BENNETT_RANDOM_SEED=0x87196caa251d0f43 \
  cargo test -p bennett-front-tests --test qubit_differential long_randomized_differential -- --ignored --exact --nocapture
```

A run is clean when it shows no wrong result, no run-time error in an accepted program and no nonzero release
in a program marked clean. Runs of 15 minutes each on a shared machine (counts differ with load):

| Front | Generator | Seed | Generated | Accepted | Declined | Compared | Wrong | Refused where expected to lower |
|---|---|---|---:|---:|---:|---:|---:|---:|
| `cb4c315` | `18c1973` | `0x87196caa251d0f43` | 7,235 | 4,892 | 2,343 | 32,136 | 21 | (not checked) |
| `49e747f` | `18c1973` | `0x87196caa251d0f43` | 6,616 | 4,459 | 2,157 | 29,344 | 0 | (not checked) |
| `30b3149` | `18c1973` | `0x87196caa251d0f43` | 6,781 | 5,050 | 1,731 | 33,316 | 0 | (not checked) |
| `30b3149` | `18c1973` | `0x0b7e11a5d1ff0007` | 6,813 | 5,064 | 1,749 | 32,968 | 0 | (not checked) |
| `663f78d` | `663f78d` | `0x87196caa251d0f43` | 6,575 | 5,234 | 1,341 | 34,376 | 0 | 29 |
| `663f78d` | `663f78d` | `0x0b7e11a5d1ff0007` | 6,578 | 5,243 | 1,335 | 34,276 | 0 | 28 |

The widened generator makes its ignored long run fail on the last column, which counts refusals, not wrong
results.

The shape behind the last column (refused where the rules accept it), witness
`merged_held_store_can_use_the_value_before_the_exchange`: `if (!q) a = 5; else { a ^= q; b += q; }` with `a`
and `b` globals. All 57 refusals counted there carried the message "a call that depends on a qubit and reads
memory that may change before its result is used" and came from the recipe that writes an object in two
regions. Accepted since; cause and fix in "Code under a qubit: merged stores and updates in loops
(2026-11-18)".

Debts, as a measure of what the front does not accept yet (declines in the last run, seed `0x87196caa251d0f43`,
then `0x0b7e11a5d1ff0007`; messages grouped over object, width and offset; most of the second line is the
generator's recipe that expects this refusal):

| Decline (each begins `not handled yet:`) | Count |
|---|---:|
| an update of a qubit variable in a loop under a qubit-dependent condition, whose value before the loop is not known and which the loop does more with than XOR into it | 809, 795 |
| a call that may write memory, controlled by a value that depends on a qubit (a call that fills memory named at the call) | 470, 470 |
| a call that depends on a qubit and reads memory that may change before its result is used (the shape above included) | 42, 45 |
| an update of a qubit variable under a qubit-dependent condition that neither XORs into it nor becomes straight-line code | 18, 22 |
| a call that depends on a qubit, whose result is not used, to a function that may write memory | 2, 2 |
| an access through a pointer that may reach bytes while they hold qubits | 0, 1 |

None of these was work for this run; the routes are in `docs/DESIGN.md` ("Code under a qubit", "Applied
calls", "Memory written under a qubit") and in "Fixes after the randomised differential test (2026-11-18)".

## Code under a qubit: merged stores and updates in loops (2026-11-18)

**A store merged at a join (step 1).** Witness `merged_held_store_can_use_the_value_before_the_exchange`
(`front/tests/tests/qubit_differential.rs`), refused on the code before this step in all three modes, seen.
Cause, read in the IR after outlining: the region `if (!q) ... else { a ^= q; b += q; }` stores into `b`, so
`b` is held before it; LLVM merged the two stores to `a` into one store at the join, after the region, of a
value the region selects. The region is outlined into a function that loads the global `a` itself. The merged
store then starts the span of `a` just before it, after the region, and the control program applies the
outlined function where its value is stored, after the exchange that moved `a`'s bytes into its shadow: the
check that the memory a call reads is unchanged between the call and that point refused it, rightly, since the
applied function would then have read bytes holding qubits. Fix (`front/src/Outline.cpp`, `outlineAround`): an
outlined region's loads of ordinary memory at an address fixed before it, which may be made there whatever
path it takes (LLVM's `isSafeToLoadUnconditionally`), are made before the region and passed in, as its loads of
qubit variables already were; the region uses the value of `a` from where it is, and reads no memory. What a
call inside the region reads is still checked: `merged_held_store_read_by_a_helper_after_the_exchange_is_refused`
(the same shape with the read of `a` in a helper the region calls) is refused by name in all three modes.
Documents: `docs/DESIGN.md`, "From C to a device program" (conditional jumps, joins and loops; the sentence
there that such code may not write memory was out of date), "Code under a qubit", "Applied calls".

A consequence, named: a region that reads bytes at a fixed address only on some paths now reads them on every
path, in the control program. Where an earlier call left those bytes holding qubits that read is the named
fault `qubit_memory_access` for the whole run, where before it was that fault inside the `apply`, for the
argument values whose path read them (a named fault either way; the front assumes such bytes ordinary where a
span starts, "Questions for the maintainer").

Capped run (`BENNETT_RANDOM_CASES=400`, seed `0x87196caa251d0f43`, `slot.sh heavy`), the same 400 cases on the
front before and after the step: accepted 330 → 332, declined 70 → 68, compared 2,164 → 2,172, wrong 0 → 0,
failures 1 → 0 (the one failure before was this refusal, which the recipe counts). Declines of "reads memory
that may change before its result is used" 2 → 0; of the update in a loop under a qubit-dependent value whose
old value would have to be kept 45 → 45.

**A nonzero release in a program marked clean.** The 15-minute run on `8cbccd1` (seed `0x87196caa251d0f43`,
index 3960, O0) found one: two regions under `q & 2` and `q & 5`, the first writing `cell0` (a loop of XORs) and
`cell1` (`^=`), the second XORing into both by values that do not read them; for the definite input 5 one
register of 8 qubits was released nonzero, with the warning that an overwrite of `cell1` keeps its old value, in
all three modes (seen). The generator's marking is right (DESIGN's accumulation rule), so the front was wrong.
Cause: LLVM forwarded the first region's value of `cell1` (a value selected at its join) to the second region's
read, so the second region writes `cell1 = c ? e ^ P : cell1` with `P` that value, not a load of `cell1`; that is
an overwrite whose old value must be cleared. It could have been, by computing it again: after the first region
`cell1` holds its known start XOR its field of the region's result. But the first region had two results, which
go through a shared fresh register and CNOTs, and after the CNOTs the front recorded `cell1`'s value as unknown;
with one result, XORed straight into the variable, the same value was already recorded. Fix
(`front/src/RegionResults.cpp`, `front/src/Taint.h`): after the pair of `apply`s around a shared register, each
variable whose start was known is recorded as holding start XOR its field (`bennett.holds`, a record for the
analysis that emits no code), so the later overwrite clears the old value by computing it again and keeps
nothing. Test: `later_region_xors_into_a_forwarded_merged_store` (the generated program as it was; one nonzero
release before the fix, none after, values equal to native C, in all three modes). The replay of index 3960
passes, its state check included. Capped run after the fix (400 cases, the same seed): accepted 332, declined
68, compared 2,172, failed 0, as before it (this case lies beyond the first 400).

**An update in a loop under a qubit-dependent value, old value known or recomputable (step 2).** Before, a
variable that code under a qubit with a loop read and overwrote, whose value before the code was neither known
nor only XORed into, was refused ("... which the loop does more with than XOR into it (its old value would have
to be kept: not built yet)"); the randomised test declined about 800 programs a seed with it. Now such a
variable is *exchanged* (`front/src/RegionResults.cpp`, `front/src/Control.cpp`, `exchangedStores`): the one
applied function of the code takes its old value as an argument and returns its final value in a word of its
own; the control program applies every exchanged variable's final value into a fresh register while all old
values are still in place (the code's other results first), exchanges each register with its variable by
CNOTs, and clears each register by the old value where it was known or by computing it again where it is a
value computed from registers unchanged since and not of the group (`recomputable`, the clearing the stores
under a qubit already use). The code's loops run inside the applied function under its one budget: one run of
the whole code per exchanged variable. Where the old value is neither, the front refuses by name with the
route (step 3). A second shape the tests met: LLVM keeps a loop's variables in registers through the loop and
stores them after it, so the loop writes nothing and a store after it may read a variable stored just before;
that read was refused ("... is used after the variable changed"), and is now computed again from registers
unchanged since (`ControlWriter::xorInto`, only when a read is stale). Tests (`qubit_differential.rs`,
`loop_overwrite_*`): an old value stored before the loop, returned by an applied call, written by earlier code
under a qubit (two exchanged variables beside an accumulator, so the group and its order are exercised), and
the promoted form; values equal to native C for every value of a three-qubit register in all three modes,
nothing released that is not 0, the state before measurement equal to the prediction (a Z on one qubit);
refused by name after a gate on the variable. Each failed on the code before (refused: four with the old
message, the promoted form with "used after the variable changed"). Documents: `docs/DESIGN.md`, "Code under a
qubit" (exchanged variables) and "Overwrites that keep the old value".

Capped run (400 cases, seed `0x87196caa251d0f43`): accepted 332 → 370, declined 68 → 30, compared 2,172 →
2,392, failed 0 → 0. Declines of the loop update 45 → 6, all now the case whose old value is neither known nor
computable again (step 3).

**A later overwrite of an exchanged value (found by the capped run of step 3 on the second seed, a defect of
step 2).** Generated case 253 of seed `0x0b7e11a5d1ff0007` (O2), refused before step 2, was accepted by it with
one register of 32 qubits released nonzero in a program marked clean: the first region exchanges `cell0`, whose
old value (`17 * q + 3`, stored before) it clears by computing it again, so `cell0` then holds the region's
final value, a function of that old value; LLVM forwards it to a second region that overwrites `cell0` (the
shape of the fix above), and clearing it there needs the old value computed again as well. `recomputable`
refused a computation that reads its own target's earlier value, although `collect` (with `knownLoads`), which
builds the clearing, computes such a read again from the value recorded for it. Fix (`front/src/Control.cpp`):
`recomputable` accepts a read whose variable has changed, or is the target, when the value recorded there can be
computed again in turn (guarded against cycles). Test: `later_region_overwrites_an_exchanged_value` (the
generated program; one nonzero release before, none after, values equal to native C, all three modes).

**The retained case (step 3).** Where an exchanged variable's old value is neither known nor computable again
(the variable was changed by a gate, or its old value was computed from a register that has changed since),
its fresh register is released as it is by `qfree`, the partial trace (the maintainer's decision to accept an
overwrite that cannot be erased and warn), with the warning naming the variable, the qubits kept each time the
store runs (one register of the variable's width, per executed overwrite) and the sentence that if the applied
function faults for some argument values, interference between those values and the rest can be lost when the
fault register is released. In one group some registers are cleared and others kept. Nothing about the loop
changes: the code is still one applied function under one budget. Tests (`qubit_differential.rs`,
`loop_overwrite_*keeps*`, each refused on the code of step 2): after a gate on the variable, all old values
different, equal in pairs, and one variable kept beside one cleared in the same loop; values equal to native C
for every value of a three-qubit register in all three modes, the warning and its count, exactly one release
that is not 0, of the variable's width, and the reduced state of the kept registers equal to the prediction
(coherence exactly between the values whose old values are equal, the preparation's phases included).
`region_whole.rs`'s refusal test of this shape is now `..._keeps_the_old_value` (old value 4 for every value of
`x`: nothing lost, one release of 8 qubits). Documents: `docs/DESIGN.md`, "Code under a qubit" no longer names
the case refused. Capped runs (400 cases each): seed `0x87196caa251d0f43` accepted 376, declined 24, compared
2,436, failed 0; seed `0x0b7e11a5d1ff0007` accepted 374, declined 26, compared 2,364, failed 0; no decline of
an update in a loop in either (the 6 left after step 2 on the first seed were recomputable, accepted since the
fix above).

Debts of these steps: each exchanged variable costs one run of the whole code (and one computation of its old
value to clear it), where one fresh register for all of them would cost one (route: one register of all
exchanged fields while they fit in 64 bits, each cleared by its old value shifted into its field); the
retained-state comparison of the randomised test (`qubit_differential/state.rs`) covers measured registers, not
bytes measured at an address, so the retained tests measure locals (route: count the cells' columns as outputs
there); a value LLVM forwards from one piece of code under a qubit to a later one is recomputed by the later
`apply` (the existing debt in DESIGN, "Code under a qubit"); the step-1 consequence named above (a fixed-address
read on every path).
