# HANDOFF

Written on 2026-11-18, about 08:10 UTC, from the old 1,890-line file; the old file's measurement records and history are in `docs/RECORDS.md`.

## State

The restart of Bennett in one workspace, grown from the thesis program outward: rotate a bit, let ordinary code depend on it,
measure. The earlier `bennett-*` repositories are a read-only archive. Vision: `docs/NorthStar.md`; working design: `docs/DESIGN.md`.
C with a few marker calls (`docs/MARKERS.md`) goes through stock `clang` 18, then `bennett-front` (C++ over LLVM 18), which writes a
*device program* (`crates/ir`: ordinary code plus quantum instructions), which the *executor* (`crates/exec`) runs. The executor is a
simulator of the model in `docs/DESIGN.md`. The device interface and counter are available.
The gate emitter compiles applied integer functions and loops for the counting device and for
the simulator with `--apply-circuit auto`. The default simulator evaluates applied functions
directly.

What works, one line each (the commit-by-commit table is in `docs/RECORDS.md`, part two):

- The device program (one data structure, reader, writer, validator), the executor with forward and reverse (undo journal) running, `apply`, gates, qubits in memory, and the CLI (`bennett run`, `bennett check`).
- C front end: ordinary C including variadic calls, 128-bit integers, struct and vector values, intrinsics, computed goto, `setjmp`/`longjmp`, return addresses, constructors and destructors; Grover over a plain C predicate on 8 bits.
- Floating point: `float`, `double`, `long double` over Berkeley SoftFloat 3e (`rt/softfloat`), checked bit for bit against hardware; `_Float16`, `__bf16`, `fmal`, conversions to and from 128-bit integers; complex helpers from libgcc 13.3.0 (`rt/libgcc`).
- C library: musl 1.2.5 vendored (`rt/musl`) at the system-call boundary, with its own startup and exit, journalled files, a run-input clock, an entropy image, signals a program sends itself.
- Real C programs against native, standard output and exit status (`experiments/c-corpus/README.md`): GCC torture tests 1,574 agree, 1 refused by name; c-testsuite 219 agree, 1 over the step budget; GCC `ieee/` 64 agree, 3 refused; none wrong or crashing.
- SQLite, unmodified, as a SQL oracle: run, reversed exactly, Grover amplitudes checked at N = 16 to 512; and applied from C to a key that depends on a qubit register (`experiments/sqlite-on-device`). Lua 5.4.9 through musl (`experiments/lua-probe`). C North Star rungs 1 to 3 pass (`experiments/northstar-ladder`).
- `apply` from C: a call whose arguments depend on a qubit, to an ordinary function, is applied whole with one budget; its writes are undone, only its result survives.
- Code under a qubit (code whose control flow or stored value depends on a qubit): code without gates writing qubit variables is one `apply` (rule B); a gate loop with an ordinary trip count stays a loop (rule C); stores into memory at a definite address keep their value in qubits at the bytes (rule A); an overwrite that cannot be erased is kept with a warning. A randomised test against native C (`front/tests/tests/qubit_differential.rs`) found no wrong result in its last runs.
- Ten automatic checks and an opt-in pre-push hook (`docs/CHECKS.md`).
- `mark` and `rewind` are gone; `f81a70a` is the last commit that contains them.

## How to check it

Run each in the foreground, from the repository root (`CARGO_BUILD_JOBS=3` on the shared laptop):

- `cargo test`: 365 passed, 0 failed, 5 ignored (each names its reason: heavy reference, generated runs, witnesses of recorded defects); about 10 s, no LLVM needed.
- `front/test.sh` (needs LLVM 18, clang 18, CMake): 345 passed, 0 failed, 22 ignored (each names its reason: the fallback oracle's waiting test, the SQLite acceptance, the North Star ladder runs, musl-linked checks that need `rt/musl/build.py` first, heavy runs, witnesses of recorded debts); it also compiles the C++ header and classifies the 88 LLVM attribute kinds.
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`: both clean on 2026-11-18 (`scripts/check.sh`: tree guards 3,642 files, 0 problems).
- `scripts/check.sh` runs all four plus the tree guards (`docs/CHECKS.md`).
- Randomised differential run, 15 minutes, ignored by default: `BENNETT_RANDOM_SEED=0x87196caa251d0f43 cargo test -p bennett-front-tests --test qubit_differential long_randomized_differential -- --ignored --exact --nocapture`.
- Applied SQLite, ignored (needs the SQLite sources): `SQLITE_DIR=<amalgamation directory> front/test.sh --test sqlite_applied -- --ignored --nocapture`.
- Library and corpus runs: `python3 rt/musl/build.py`, then `python3 rt/musl/test.py`; corpora in `experiments/c-corpus/README.md`.

## The model in ten lines

`docs/DESIGN.md`, "The model", has the definitions and the argument; in short:
1. A device program is ordinary code that issues the operations of the standard quantum circuit model, plus one construction, `apply`.
2. A qubit is a two-level system; a register is a numbered group of qubits read as an integer; a basis configuration is one definite value of every register; a state assigns each configuration an amplitude.
3. Ordinary code is integer arithmetic, memory, jumps, calls, console output and stop; every step is a total function of the machine state, with named faults (codes 1 to 31) where C would be undefined.
4. `qalloc` gives a fresh register of qubits, all 0; a gate is a unitary on the amplitudes, with control qubits; rotation angles are any finite `f64`, which a device with a finite gate set approximates.
5. `measure` turns qubits into an ordinary value; a gate or a measurement may name its qubit by an ordinary memory address (a derived form, defined by its expansion).
6. `qfree` is the partial trace: the register is dropped unexamined, whatever it holds, and the other qubits keep the state they have without it; it never faults.
7. Ordinary code never sees a qubit: a `load` or `store` of a byte that holds qubits is the named fault `qubit_memory_access` (code 17).
8. `apply @f(args) -> out, fault budget B` is the one bridge from qubits to ordinary code: for each definite value of the argument registers, run `@f` (ordinary code) for exactly `B` steps on a private copy of memory, XOR its result into `out` (or its fault code into `fault`), and discard the private memory.
9. `B` is one bound for the whole applied function, its loops and calls included; a device can carry it out by keeping each previous state, copying the result out and running the steps backwards (about `B + 1` steps forward plus the copy and the inverse steps).
10. Forms beyond these (`call_indirect`, `setjmp`, `longjmp`, `exit`, qubits in memory) are defined by a straightforward expansion into them; the compiler inserts no measurement the program did not write.

## Decisions in force

Newest first; quotes are the maintainer's words.

- **2026-11-18, the next target.** The first gate-generation target is an ideal machine with one- and two-qubit gates and the Toffoli gate, qubits allocated and measured on demand; the benchmark is Shor's algorithm written as plain C, compared with hand-built circuits; loops come early.
- **2026-11-17, code under a qubit (about 15:35 UTC).** "i agree with the fable note. I like the philosophy of Soeren Wilkening: 'everything is quantum: if it is in a classical config, and only classical stuff happens, we can approx it as classical. as soon as quantum touches it it becomes quantum' measurement is the antithesis of that philosophy." So: a store whose value or execution depends on a qubit makes the bytes hold qubits (the value is applied into a fresh register and exchanged with the bytes by gates; the register is cleared when its old content can be recomputed); an overwrite that cannot be erased is accepted, the old value kept in spare qubits released by `qfree`, with a warning and the count; measuring bytes at an address makes them ordinary memory again; the compiler inserts no measurement anywhere.
- **2026-11-17, loops under a qubit.** A loop with an ordinary, definite trip count may keep one small `apply` per pass; code without gates whose control flow depends on a qubit is one `apply` with one budget; the front never creates a loop, counter or bound that is not in the C; a loop containing gates keeps a trip count written as an ordinary value in the C, its ordinary part one applied function returning a table of control bits, cleared by the same `apply`; no unrolling.
- **2026-11-17, memory written under `apply` (about 13:40 UTC).** "ok I accept rule 1 in the following sense: sqlite. I want to be able to apply sqlite. I understood that to be entirely possible by ensuring all memory allocs are *also* simulated and applied": writes made by code run under `apply` do not survive it, only the returned value does; the runtime (allocator, files, clock, entropy) is ordinary device code, so it is undone with the `apply`.
- **2026-11-17, step bounds.** "I 100% definitely require the capability to run unbounded classical loops depending on input under apply." Inside `apply` there is exactly one bound, `budget B`, for all its steps, loops, nested loops and calls included, never one per loop; `B` may be computed at run time. A fixed step bound outside `apply` is not a limitation to state ("same holds for normal computation").
- **2026-11-17, derived forms and angles.** "If the front emits more, that is also probably fine, as long as the map to 1-9 + apply is obvious, or straightforward." Rotations by any angle are in the gate set: "there really is no harm in arbitrary angles".
- **2026-11-17, goals, not started.** "a lean proof of the executor is a medium term goal"; "partial functions are probably fine in the standard way" (on each step being a total function of the machine state).
- **2026-11-17, the model.** `qfree` is the partial trace ("qfree can only mean partial trace. I don't see how measuring is stable here: what if nonzero has tiny prob?"); `mark` and `rewind` are removed (last commit with them `f81a70a`; their proved narrow static form, each mark paired with its rewind in the text, can come back later with its test attached); gates and measurements on memory bytes keep their plain form ("ok keep plain form"); ordinary code touching a byte that holds qubits is a named fault ("yes, it should be a fault to read qubits").
- **2026-11-16, `apply`, `qalloc` and `qfree` are not retired** until the maintainer decides otherwise: the forms may suit front ends for other LLVM languages.
- **2026-11-16, the coherent class** (updates guaranteed to stay coherent) stays as documented in `docs/DESIGN.md`, "What is promised about coherence"; it may never shrink, and growing it waits for an oracle that compares phases as well as weights. NaN payloads: a floating-point result that differs from native only in which input NaN's payload it carries counts as agreement, by a rule that allows exactly this.
- **2026-11-16, the device.** The clock is a start value given to the run plus the number of clock reads; entropy is a seeded byte image given to the run; both are inputs of the run. `measure iN [ADDR]` takes an address. Fault codes are 5 bits wide; every `longjmp` that finds no valid record is `bad_longjmp` (16); a record held in qubits that disagree stays a fault.
- **2026-11-16, C.** All six complex helpers come from libgcc (so the device equals the corpus's native reference); calls through function pointers keep the full check; a stale `setjmp` record keeps today's behaviour; a copied `sigjmp_buf` follows glibc; return-address values at and beyond the outermost frame stay as they are; the floating-point environment stays a named fault (rounding modes and flags become real through SoftFloat's state later); the exit status is not masked on the device; `front/include/bennett.h` is includable from C++.
- **2026-11-16, how work is done.** Ten automatic checks (`docs/CHECKS.md`) and test-writing rules (`docs/DESIGN.md`, "Tests").
- **2026-11-15, floating point is Berkeley SoftFloat 3e** behind the libgcc names, swappable at link time; Bennett.jl's branch-free routines stay a later cheaper alternative, to be checked bit for bit against it.
- **2026-11-15, the C runtime boundary.** Console output from C is `void __bennett_effect(char)` (an ordinary call); `exit v` ends the run from anywhere (`_Noreturn void __bennett_exit(int)`); the C library is musl 1.2.5 at the system-call boundary, anything not implemented traps by name. Zero-filled data is a compact `zero N` item in the text form.
- **2026-11-13.** Coverage must end at 100%: no refusals in the end unless physics forbids it, and then the reason is stated; a "not handled yet" is a debt. Classical results are exact; coherence is guaranteed for a documented class and for anything un-run, elsewhere unspecified with a warning. Channels (partial trace, measurement by effects, noise) belong to the finished Bennett and nothing built now may make them impossible to add. The North Star was reviewed (13 kept, 10 reworded, 3 struck: `docs/NorthStar.md`).

## In flight

The device interface, step tests and loop emitter have landed. The gate compiler follows
definite control, uses a single clock for general qubit-dependent jumps, and saves decisions
for one proved loop. The follow-up on `wip/sol62-loops-m2` checks shift and multiply operands
across the whole applied function before using the budget and reports the weight of emitted
faults, including with nonzero fault destinations. The emitted simulator includes circuit
scratch in its peak qubit count. The CLI checks the gates sent by the controller against a
hand-counted applied circuit. The 8-bit Collatz
bit runner checked all 256 inputs at 253 budgets with both constructions. The simulator matched
the reference amplitudes exactly after each applied circuit for widths 2 to 5. The nested loop
returns 1 at budget 14 and writes `budget` at 13 for `x = y = 1`.

- Device support: an interface between the executor and its quantum simulator (so the simulator can be replaced); a counting device that only counts qubits and gates; rewriting of gates with many controls for a machine with one- and two-qubit gates and the Toffoli gate; a bit-level circuit runner that checks reversible gate lists against independent references.
- `wip/sol62-stepspec` (landed): tests pinning an exact statement of what an applied function does for one definite input, step by step, and a change that makes the slots of a new frame zero.
- Never to be landed: `wip/fallback-step3`, `wip/sol-mutants-machine`, `wip/sol-mutants-machine-partial` (they build on the removed `mark`/`rewind` machinery).

## Next steps

In order:
1. **Controls for qubit-dependent choices**: prove when a choice can become a control on the gates
   of its bodies without instruction dispatch.
2. **Applied calls and arithmetic**: extend gate compilation with call frames, addresses and the
   multiply and remainder operations needed for modular arithmetic.
3. **Shor's algorithm for 15 and 21 from plain C**, as the benchmark against hand-built circuits (qubits, gates, depth).
4. **Applied memory**: add private memory and stack operations, with the corresponding first-fault rules.
5. Then what the old plan left undone: the table of control bits for a gate loop deciding on carried values (question 5); rule A at an address that depends on a qubit (plan: declare the region, then exchange one addressed element, cost the region's size times the element's width per access); a stateful library whose writes must survive (k applied calls on qubit-held memory run with nothing kept when the calls' arguments are kept and memory starts known: k(k+1)/2 evaluations call by call, k as one applied function); a second implementation of the device language (grow the reference that runs `apply` by its construction, `crates/exec/tests/compute_copy_reference/`); the floating-point environment through SoftFloat's state, `__float128` and the remaining intrinsics; the runtime debts below; Grover over a SQL query written in plain C; the optimiser and cost model (`opt`, not started).
6. The next independent testing pass starts at the first commit in "Which commits no independent check has covered".

## What is refused, and what removes it

| Refused today | What removes it |
|---|---|
| Emitted `apply` using calls, private memory, stack operations, remaining arithmetic or explicit instruction faults | Gate compiler routes in `docs/DESIGN.md`, "Not yet" |
| Remaining runtime operations: locks, links, directory enumeration, block transport, permissions and metadata, image loader, allocation exhaustion | The runtime debts below |
| A call through another prototype where x86-64 does not determine what is read (upper half of a 32-bit value, a missing argument that is read, an integer from a floating-point register, a variadic floating-point argument without `%al`, a narrow argument not extended as the callee relies on); `musttail` with a by-value argument | Nothing: the native result is not determined (`docs/DESIGN.md`, "Calls through a prototype") |
| A qubit-dependent call of a function that uses `setjmp`; a `setjmp` in a function with qubit variables; a function that uses `stackrestore` run coherently | The front's own effect summary of `setjmp`; resumption edges from every later call and `longjmp` in its facts (`docs/DESIGN.md`, "Non-local jumps") |
| `__float128`; `powi` on `_Float16`/`__bf16`; `#pragma STDC FENV_ACCESS`; strict (constrained) floating point; non-default rounding modes and flags; a `volatile` vector | The floating-point environment through SoftFloat's state |
| A floating-point operation that reads the subnormal mode in a function promising a non-IEEE mode (`-fdenormal-fp-math=preserve-sign`, `positive-zero`, `dynamic`, or the `-f32` forms) | The same, compared with native code run under MXCSR DAZ/FTZ |
| Stores into memory in code under a qubit that also has gates; a held object of more than 8 bytes; an access touching held bytes only in part; a position not constant outside code applied whole; an object the front cannot name (a pointer loaded from memory); qubit variables in structs, heap objects or passed to functions | Routes in `docs/DESIGN.md`, "Memory written under a qubit": predicate whole stores into the shadow; one shadow per 8 bytes; none built |
| Pointers that may reach the same bytes as a held object (two objects written under a qubit that may share bytes; an ordinary access through another pointer; an escaped `restrict` copy); a call that may read or write held bytes, a callee that measures them included; a second shadow of bytes another write under a qubit still holds | A check of the addresses at run time choosing one shadow or two; ending the span before a call that only measures; one shadow from the earliest write. Today: a static function whose calls pass distinct objects, or `restrict` |
| An access at an address that depends on a qubit | Next step 5 (declare the region: a language choice, question list below) |
| A gate loop whose number of passes depends on a qubit, or whose passes decide on qubit-dependent values carried between them; a gate on what its condition reads | Question 5 (table of control bits); for a gate on the condition's register, one control qubit computed once and cleared by computing it again (`docs/DESIGN.md`) |
| An update of a qubit variable under a qubit-dependent condition that neither XORs into it nor becomes straight-line code | Routes in `docs/DESIGN.md`, "Code under a qubit" |
| A qubit-dependent value used by ordinary code or printed; ordinary code reading bytes that hold qubits (the front asks for `bennett_measure_bytes`); measurement under control | Channels; the programmer's own measurement |
| A call that fills memory named at the call, controlled by a qubit-dependent value; a qubit-dependent call whose result is unused to a function that may write memory; two such calls in one computation when one may write memory; a call that reads memory that may change before its result is used (when a write could reach it) | Nothing for the unused call (applied, it would do nothing); one register per call for two calls; compare the call's reach with each intervening write; none designed for a call that fills memory |
| Quantum operations in `main` of a program linked with musl | Classify control functions through indirect calls, or a startup that calls `main` directly |
| Non-empty inline assembly; code without source; integers wider than 128 bits | An emulator of the real machine, compiled as an ordinary program; a type legaliser for wide integers |

## Debts

One line each, with the route:
- A second implementation of the device language (the removed reference modelled the removed machinery; rungs 1 and 2 and several tests lost it). Route: grow the `apply` reference (small subset today: integer operations, byte loads and stores, fixed call frames, jumps, returns), instruction by instruction.
- The fallback oracle's `user_write` acceptance waits (a call that fills memory named at the call, under a qubit). Route: none designed.
- Rule A at memory, listed refusals above. Not refused, documented: where LLVM's bound is wider than the C's writes (a 2- or 4-byte `memcpy` is bounded at 6) the extra bytes hold qubits with their old values; a second function's store under a qubit into bytes an earlier call left holding qubits is the fault `qubit_memory_access` (question 3).
- Cost of code under a qubit: each exchanged variable costs one run of the whole code (route: one register of all exchanged fields while they fit in 64 bits); a value LLVM forwards between pieces of code under a qubit, or a join the code leaves for later code, is computed again by the later `apply`; a fixed-address read is made before a region only for globals the front shows ordinary (route: the same proof for frames and heap).
- The retained-state comparison of the randomised test covers measured registers, not bytes measured at an address (route: count the cells' columns as outputs).
- Refusal messages wider than what is unsafe: a measuring callee, a second shadow and an escaped `restrict` copy; the message for a measuring callee does not say it measures.
- An overwrite of a qubit variable after its value became unknown takes the retained form where an in-place inverse would do (`if (r) y += 2`); route: invert the predicated store in place when the update is a bijection on a condition that does not read `y`.
- A `_Bool` qubit costs 8 qubits; a qubit in memory is a whole byte, 8 qubits (route: cells of single bits). The region budget counts device instructions, not a unit the source fixes. An aggregate with a pointer is returned through memory (refused under qubit control at `-O2`); a packed struct of 129 bits is an `i136` (refused).
- Floating point: the documented-half and bfloat software conversion differences have ignored witnesses (`rt/softfloat/README.md`); which NaN comes back when several operands are NaNs is unspecified; `long double` encodings no arithmetic produces are read as x87 reads them, costing about 10% more steps on the long-double tests; the grouped checks of `front/tests/tests/complex.rs` count any two NaNs as equal.
- Runtime: separate per-invocation inputs (route: a symbol/relocation map and an initial-memory loader); file operations not carried (locks, links, directory enumeration, extended attributes, `renameat`/`linkat`, file-backed mmap: each must be real state, compared with native); namespace and metadata (root ownership, fixed device identity, zero timestamps, no credentials, no descriptor limit, a removed working directory names nothing); allocation exhaustion and reclamation (stage allocations before publication, reference counts); block transport and sleeping (`nanosleep` and timers trap; define sleep against the input clock first).
- `setjmp`: a stale record can match another activation at the same depth (route: an activation counter); a partial `sigjmp_buf` copy can select the wrong continuation (ignored witness `a_partial_record_copy_preserves_the_glibc_continuation`; route: keep glibc's byte-level copy semantics in the lowered environment); equal-priority constructor order across separately compiled objects (route: lower startup per unit before linking); return-address values at the outermost frame differ from native.
- Applied calls: nothing in the front checks that calls arrive as calls (route: name an inlined callee from its debug location; mark calls in control functions `noinline` before the front's optimiser); the warning follows direct callees only, so no warning proves nothing.
- Smaller: a null dereference does not fault; `bennett run --arg` refuses parameters wider than 64 bits; native `-O2` and the device may disagree where C leaves a result open (`fmin` of two zeros); `printf("%a")` of a subnormal (question 11); the observer's API names and the experiment READMEs still use the older vocabulary for the separate runs of an `apply` (question 9).
- Checks: automatic changed-line mutation is Rust-only, so C++ and script changes use recorded seeded edits (route: an established C++/script mutation engine with diff selection); whole-step mutation and cold pre-push builds have no seconds-long guarantee (route: cache dependency artifacts, select proven relevant tests); `scripts/mutate-diff.py` crashes on a diff with no Rust mutants; attribute inventory covers the measured envelope only (`inreg`, `preallocated`, `inalloca` unmeasured; no AVX-512 on this CPU); `cargo clippy -p bennett-front-tests --all-targets` reports two `unwrap` uses in `front/tests/tests/complex.rs`; an unused import warning in `front/tests/tests/thesis.rs`.
- A passing suite is evidence, not proof (blind spots found by mutation checks more than once); the numbers in `docs/AUDITION.md` are each proposal's own and were not recomputed.

## Questions for the maintainer

Each is taken in the cheapest way to reverse, as stated, until answered. Last 2026-11-18.

1. **Releasing bytes that hold qubits without measuring them.** Today the only way back to ordinary memory is a measurement. Add an instruction that drops them as `qfree` drops a register? It is a new instruction, so yours to decide. Nothing taken.
2. **Needing a definite value from bytes that hold qubits** (printing, a jump, the exit code). Taken: the compiler refuses and asks the programmer to write the measurement. Alternative: the compiler inserts one, which your 15:35 decision on 2026-11-17 rules out for stores. Recommend: keep.
3. **When an earlier call may have left bytes holding qubits where a later store under a qubit starts.** Taken: the front assumes they are ordinary; if they hold qubits the run stops with the named fault `qubit_memory_access`. (b) exchange without the old-value load (always correct, but old values are then kept with a warning and loops that overwrite are refused); (c) a flag per object in ordinary memory, both forms written. Recommend: keep now, (c) when a program needs it.
4. **A function the host may call with pointer arguments while memory holds qubits.** The front cannot see the host's calls, so two pointers may name the same bytes. Taken: refuse by name where that could reach bytes holding qubits. (b) trust the entry functions the tests name; (c) check the addresses at run time. Recommend: keep now, (c) when needed.
5. **A loop that applies a gate in some passes, where which passes depends on a value computed from a qubit and carried from pass to pass** (`t = x; for (i = 0; i < n; i++) { if (t & 1) h(y); t = step(t); }`, `n` an ordinary run-time value). Refused today. Needed: a table of control bits, one per pass, whose width is `n`, and a way to name pass `i`'s bit. (a) build it when `n` is a constant of at most 64 passes times gate sites; (b) for any `n`, one small `apply` per pass recomputing that pass's decision (about `n²/2` passes of work, each with its own budget); (c) the table in qubit-held bytes after rule A at addresses. None adds a bound per loop. Nothing taken. Recommend: (a) first, then (b).
6. **Results of code under a qubit wider than 64 qubits** (a register holds at most 64). Taken: one applied function computing every 64-bit word, each word applied separately (twice the whole code per word). (b) refuse wider results; (c) allow registers wider than 64, a change of the language. Recommend: keep until cost shows otherwise, then (c).
7. **Applied calls, three choices taken.** A call with qubit-dependent arguments whose result is unused, to a function that writes memory: refused (applied, it would do nothing); recommend keep. Two such calls in one computation when one writes memory: refused; recommend build one register per call when a program needs it. The warning's reach (direct callees only): recommend extend if a real program misses it.
8. **A derived form naming bits at addresses among an `apply`'s arguments**, defined as copy into a register, apply, copy back? Today a program does that by hand (an applied function cannot read qubit-held bytes). Recommend: yes, once C functions take qubits through pointers.
9. **Renaming names left from the removed machinery**: the observer's API (`ApplyJournal::branches`, `BranchJournal`, `branches_total`, `branches_present`), the front's test helper `branches`, `Stats::leftover_qubits`, and the experiments' READMEs (most in `experiments/journal-content`) still use the older vocabulary for the separate runs of an `apply`. Taken: unchanged. Recommend: rename to `runs` / `configurations` / `kept_qubits` in a step that touches the observer anyway.
10. **Fault codes 11 and 15** (both removed) stay unused rather than renumbering, since a program may have measured one. Recommend: keep.
11. **`printf("%a")` of a subnormal double**: the device prints musl's spelling (`0x1p-1074`), native glibc prints `0x0.0000000000001p-1022`; both conform to C. Taken: unchanged, witness ignored (`front/tests/tests/printf_hex_float.rs`). Recommend: follow glibc by an overlay of musl's hexadecimal path in `fmt_fp` (low priority), or record musl's spelling as the device's.
12. **Two options doing the same thing**: `--machine-meaning` and `--allow-region-faults` (stop at an `apply` faulting for some argument values). Taken: both kept. Recommend: keep one.
13. **NaN payload rule.** Two NaN results agree when each carries the payload of one of the operands' NaNs or the default NaN's, sign free (needed: without the default NaN 1,476 of 164,096 `__divdc3` tuples fail). Taken. Recommend: keep.

- **Single-loop proof.** Taken: recognise one conditional test and straight continuing and
  exiting routes with provably lowerable integer operations. Recommend broadening the proof
  to additional total control-flow shapes only when a measured program needs it.

- **Simulator entry point for emitted gates.** Taken: opt in with `--apply-circuit auto`
  or `--apply-circuit step-machine`. Recommend keeping direct evaluation as the default while its
  instruction coverage and strict fault diagnostics exceed the gate compiler's.

- **Constant operands in emitted arithmetic.** Taken: prove a single value across all
  incoming edges and visits before selecting a shift or fixed-factor multiply. Recommend
  keeping the proof conservative until a program needs more facts about definite control.

## Which commits no independent check has covered

- `58d6e4b` (a fix: only its own tests and the 15-minute randomised runs on it have covered it). The next independent testing pass starts there.
- `5fc7386` (the new-frame change) and `90f7305`, `6155161` (test-only: the step-rule tests); all three came from `wip/sol62-stepspec`, and no independent check has covered them yet.
- Documents-only commits (`babaa5c`, `fc33dc9`, and the ones that carry this file) and the generator commits, which are test-only and had their sensitivity shown on a deliberately changed scratch front.
- Everything else in code under a qubit has had an independent testing pass or the 15-minute randomised runs; the list, commit by commit, is in `docs/RECORDS.md`, part two ("The top of the old HANDOFF").

## Where the records are

- `docs/RECORDS.md`: dated measurements, testing passes and fixes (part one); the old plans, decisions and questions (part two).
- `docs/DESIGN.md` (the model, decisions in code, tests), `docs/SYNTAX.md` (text form), `docs/MARKERS.md` (marker calls), `docs/CHECKS.md` (automatic checks), `docs/NorthStar.md` and `docs/northstar-review.md` (vision and its review), `docs/AUDITION.md` (design audition), `docs/SALVAGE.md` (what the earlier workspace offers).
- Experiments: `experiments/c-corpus` (corpora), `sqlite-on-device`, `northstar-ladder`, `journal-content`, `lua-probe`, `libc-probe`; runtimes: `rt/musl`, `rt/softfloat`, `rt/libgcc` (each with a `README.md`).
- Git keeps everything removed; `f81a70a` is the last commit with `mark` and `rewind`.
