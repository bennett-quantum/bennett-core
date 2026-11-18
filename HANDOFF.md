# HANDOFF

Written on 2026-11-18, about 08:10 UTC, from the old 1,890-line file; the old file's measurement records and history are in `docs/RECORDS.md`.

## State

The restart of Bennett in one workspace, grown from the thesis program outward: rotate a bit, let ordinary code depend on it,
measure. The earlier `bennett-*` repositories are a read-only archive. Vision: `docs/NorthStar.md`; working design: `docs/DESIGN.md`.
C with a few marker calls (`docs/MARKERS.md`) goes through stock `clang` 18, then `bennett-front` (C++ over LLVM 18), which writes a
*device program* (`crates/ir`: ordinary code plus quantum instructions), which the *executor* (`crates/exec`) runs. The executor is a
simulator of the model in `docs/DESIGN.md`. The device interface and counter are available.
The gate emitter compiles applied integer functions with one step budget for the whole `apply`.
Qubit-dependent choices and loops with a proved pass count can use control gates directly. The counting device uses it, and the simulator offers it through
`--apply-circuit auto` or `--apply-circuit step-machine`. The default simulator evaluates applied functions
directly. Integer calls copy their frames and use the shared budget. The emitter refuses
recursive calls, unproved indirect targets, excessive call expansion, private memory
and stack operations, switches without a
proved definite selector, `or`, division
and remainder, shifts without a proved constant amount, and instructions
such as `fault` that still need named fault handling. See `docs/DESIGN.md`, "Not yet".

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

- `cargo test`: 387 passed, 0 failed, 5 ignored (each names its reason: heavy reference, generated runs, witnesses of recorded defects); about 10 s, no LLVM needed.
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

- **2026-11-18, proved loop counts.** The gate emitter may use a pass count proved from the program at any nesting depth. Within an applied function only one loop may use a count derived from `budget B`. An inner schedule may never be repeated for each outer slot using budget-derived counts.
- **2026-11-18, dropping bytes that hold qubits.** Add a device instruction that drops the bytes' qubits unexamined by partial trace, as `qfree` drops a register.
- **2026-11-18, rotation angles.** A rotation may take an ordinary value computed by the control program as its angle. Add this to the device language and C markers.
- **2026-11-18, in-place form.** The device language may add a form that replaces a register's value with `f` of it given the inverse `g`, after the unfused Shor program is measured. "in place is an extra form is fine; I would hope that optimising passes will get so good in future they find these opts themselves."
- **2026-11-18, wide registers and held objects.** The device language will allow registers wider than 64 qubits and objects that hold qubits over more than 8 bytes, before Shor above 64 bits.
- **2026-11-18, addressed arguments to `apply`.** When C functions can take qubits through pointers, add an argument form for bits at an address, defined by copying into a register, applying the function, then undoing the copy.
- **2026-11-18, the coherence guarantee.** The class may grow to include the named shapes covered by the state-comparing randomised test and may never shrink. Each shape is marked proved or tested only. "i prefer that proofs are additionally supplied for every added shape. This must be listed as future work. any shape admitted without proof must generate a warning or sth so we know it might become unsound (warning only in case of missing proof)"
- **2026-11-18, writes inside an applied call.** Undoing the writes is a permanent rule of the language. Write calls that must share state as one applied function.
- **2026-11-18, Shor benchmark.** Both Toffoli count and peak qubits must be within a factor of two of a textbook-class hand-built circuit using the same registers. Windowed circuits are the stretch target. "i hope once our optimisation passes get better we can automatically exploit measure reuse etc and actually match the textbooks"
- **2026-11-18, proof paper.** Publish it under `docs/` after marking the sections on removed machinery and settling the licence of its proof style file.
- **2026-11-18, gate loops with a carried value.** Build the table of control bits first for constant counts whose table fits in 64 bits. Then cover any ordinary count by recomputing the decision with one small `apply` per pass ("definitely exted to (b) as well").
- **2026-11-18, definite values from bytes holding qubits.** The front refuses ordinary reads and asks for a measurement written by the programmer.
- **2026-11-18, bytes an earlier call may have left holding qubits.** A new store under a qubit starts by reading the bytes as ordinary memory, which faults as `qubit_memory_access` if they still hold qubits.
- **2026-11-18, pointer arguments from the host.** Refuse by name where pointers to an externally callable function may alias bytes that hold qubits.
- **2026-11-18, applied-call diagnostics.** Keep the refusals for an unused result from a call that may write memory and for two such calls in one computation when one may write. Keep the warning at direct callees, extending it and building the per-call register route as programs need them.
- **2026-11-18, strict mode options.** Keep both `--machine-meaning` and `--allow-region-faults`.
- **2026-11-18, NaN payload agreement.** Two NaN results agree when each carries a NaN payload from an operand, quieted if needed, or from the default NaN. The signs may differ.
- **2026-11-18, hexadecimal printing.** Match glibc's `printf("%a")` for subnormal doubles. Low priority.
- **2026-11-18, fault codes.** Leave 11 and 15 unused and preserve existing numbers.
- **2026-11-18, names from removed machinery.** Rename the remaining names when changing that code for another reason.
- **2026-11-18, unlanded branches.** The advisor lists each and deletes those that only concern removed machinery.
- **2026-11-18, large files.** Split each only when it is next changed for another reason.
- **2026-11-18, pebbling repository.** Keep its history as it is.
- **2026-11-18, the next target.** The first gate-generation target is an ideal machine with one- and two-qubit gates and the Toffoli gate, qubits allocated and measured on demand; the benchmark is Shor's algorithm written as plain C, compared with hand-built circuits; loops come early.
- **2026-11-17, code under a qubit (about 15:35 UTC).** "I like the philosophy of Soeren Wilkening: 'everything is quantum: if it is in a classical config, and only classical stuff happens, we can approx it as classical. as soon as quantum touches it it becomes quantum' measurement is the antithesis of that philosophy." So: a store whose value or execution depends on a qubit makes the bytes hold qubits (the value is applied into a fresh register and exchanged with the bytes by gates; the register is cleared when its old content can be recomputed); an overwrite that cannot be erased is accepted, the old value kept in spare qubits released by `qfree`, with a warning and the count; measuring bytes at an address makes them ordinary memory again; the compiler inserts no measurement anywhere.
- **2026-11-17, loops under a qubit.** Code without gates whose control flow depends on a qubit is one `apply` with one budget. A loop containing gates keeps the ordinary trip count written in C. The front preserves the loop as written. The 2026-11-18 decision specifies when to compute a table of control bits and when to recompute a decision on each pass.
- **2026-11-17, memory written under `apply` (about 13:40 UTC).** "ok I accept rule 1 in the following sense: sqlite. I want to be able to apply sqlite. I understood that to be entirely possible by ensuring all memory allocs are *also* simulated and applied": writes made by code run under `apply` do not survive it, only the returned value does; the runtime (allocator, files, clock, entropy) is ordinary device code, so it is undone with the `apply`.
- **2026-11-17, step bounds.** "I 100% definitely require the capability to run unbounded classical loops depending on input under apply." Inside `apply` there is exactly one bound, `budget B`, for all its steps, loops, nested loops and calls included, never one per loop; `B` may be computed at run time. A fixed step bound outside `apply` is not a limitation to state ("same holds for normal computation").
- **2026-11-17, derived forms and angles.** "If the front emits more, that is also probably fine, as long as the map to 1-9 + apply is obvious, or straightforward." Rotations by any angle are in the gate set: "there really is no harm in arbitrary angles".
- **2026-11-17, goals, not started.** "a lean proof of the executor is a medium term goal"; "partial functions are probably fine in the standard way" (on each step being a total function of the machine state).
- **2026-11-17, the model.** `qfree` is the partial trace ("qfree can only mean partial trace. I don't see how measuring is stable here: what if nonzero has tiny prob?"); `mark` and `rewind` are removed (last commit with them `f81a70a`; their proved narrow static form, each mark paired with its rewind in the text, can come back later with its test attached); gates and measurements on memory bytes keep their plain form ("ok keep plain form"); ordinary code touching a byte that holds qubits is a named fault ("yes, it should be a fault to read qubits").
- **2026-11-16, `apply`, `qalloc` and `qfree` are not retired** until the maintainer decides otherwise: the forms may suit front ends for other LLVM languages.
- **2026-11-16, the coherent class** (updates guaranteed to stay coherent) is documented in `docs/DESIGN.md`, "What is promised about coherence", and may never shrink. The 2026-11-18 decisions specify how to grow it and the NaN payload rule.
- **2026-11-16, the device.** The clock is a start value given to the run plus the number of clock reads; entropy is a seeded byte image given to the run; both are inputs of the run. `measure iN [ADDR]` takes an address. Fault codes are 5 bits wide; every `longjmp` that finds no valid record is `bad_longjmp` (16); a record held in qubits that disagree stays a fault.
- **2026-11-16, C.** All six complex helpers come from libgcc (so the device equals the corpus's native reference); calls through function pointers keep the full check; a stale `setjmp` record keeps today's behaviour; a copied `sigjmp_buf` follows glibc; return-address values at and beyond the outermost frame stay as they are; the floating-point environment stays a named fault (rounding modes and flags become real through SoftFloat's state later); the exit status is not masked on the device; `front/include/bennett.h` is includable from C++.
- **2026-11-16, how work is done.** Ten automatic checks (`docs/CHECKS.md`) and test-writing rules (`docs/DESIGN.md`, "Tests").
- **2026-11-15, floating point is Berkeley SoftFloat 3e** behind the libgcc names, swappable at link time; Bennett.jl's branch-free routines stay a later cheaper alternative, to be checked bit for bit against it.
- **2026-11-15, the C runtime boundary.** Console output from C is `void __bennett_effect(char)` (an ordinary call); `exit v` ends the run from anywhere (`_Noreturn void __bennett_exit(int)`); the C library is musl 1.2.5 at the system-call boundary, anything not implemented traps by name. Zero-filled data is a compact `zero N` item in the text form.
- **2026-11-13.** Coverage must end at 100%: no refusals in the end unless physics forbids it, and then the reason is stated; a "not handled yet" is a debt. Classical results are exact; coherence is guaranteed for a documented class and for anything un-run, elsewhere unspecified with a warning. Channels (partial trace, measurement by effects, noise) belong to the finished Bennett and nothing built now may make them impossible to add. The North Star was reviewed (13 kept, 10 reworded, 3 struck: `docs/NorthStar.md`).

## In flight

Call lowering is on `wip/sol62-m4`. Both emission modes matched direct evaluation for
two continuations into a copied callee and two visits to one site. Stack tests cover
the seven-byte and eight-byte entry cases at budget zero, nested frame reservations,
and bad indirect addresses and signatures. Unsigned bounds keep the high bits of
narrowly bounded products at literal zero, including in a 128-bit destination. Qubit-held
products use column reductions with their own work wires cleared. Their Toffoli counts
are checked through a full 64-by-64-bit product.

Control-gate lowering and its review fixes landed in `e34d256`. Operand analyses
recover constants for whole-function checking on untaken edges. Only reachable
predecessors contribute to the first proof of each operand, which is the proof used to
emit gates. The analyses for bypassed blocks propagate unknown inputs before supplying
a proof and can follow literal decisions within that code. Conditional
routes that would duplicate shared code fall back to the step machine. The overlap check
follows only the selected edge of a literal decision. A proved zero-test
exit carries the zero into its continuation. The six-bit outer and inner draining example
schedules 12 body passes and 1,963 gates. A zero-shift regression checks that a
nonzero word keeps running to the budget. The tests compare
amplitudes for a choice inside a definite-count loop, unequal routes to a shared return, and
shift loops of widths 2 to 6, including a nested example. The bit-level runner checked all 256
inputs at budgets 0 through 45 for the 8-bit shift loop with both control gates and the step
machine. The counts table in DESIGN records both constructions.

The step specification in `docs/APPLY-STEP.md`
landed in `81a19f4` and is linked from DESIGN and the step tests. The loop emitter landed in
`506788f`, `09fdd28`, `aa6c733` and `28a1c00`. It follows definite control, uses one clock
for general qubit-dependent jumps, saves decisions for one proved loop, and runs on the counting
device and on the simulator. The fixes check shift and multiply operands across the whole applied
function before using the budget and report emitted fault weights even with nonzero fault destinations.
Simulator peak qubit counts include circuit scratch, the counting CLI test checks emitted gate totals,
and the scratch allocator accepts the final representable wire range. The 8-bit Collatz bit runner
checked all 256 inputs at 253 budgets with both constructions. The simulator matched
the reference amplitudes exactly after each applied circuit for widths 2 to 5. The nested loop
returns 1 at budget 14 and writes `budget` at 13 for `x = y = 1`.

- Device support: an interface between the executor and its quantum simulator (so the simulator can be replaced); a counting device that only counts qubits and gates; rewriting of gates with many controls for a machine with one- and two-qubit gates and the Toffoli gate; a bit-level circuit runner that checks reversible gate lists against independent references.
- `wip/sol62-stepspec` (landed): tests pinning an exact statement of what an applied function does for one definite input, step by step, and a change that makes the slots of a new frame zero.
- Never to be landed: `wip/fallback-step3`, `wip/sol-mutants-machine`, `wip/sol-mutants-machine-partial` (they build on the removed `mark`/`rewind` machinery).

## Next steps

In order:
1. Emit unsigned remainder and its divide-by-zero fault.
2. Check mulmod against direct evaluation and record the gate counts.
3. **Shor's algorithm for 15 and 21 from plain C.** Measure the unfused program first. Compare Toffoli count and peak qubits with a textbook-class hand-built circuit using the same registers, aiming for a factor of two or less on both. Record depth too.
4. **Applied memory**: add private memory and stack operations, with the corresponding first-fault rules.
5. Then what the old plan left undone: rule A at an address that depends on a qubit (declare the region, then exchange one addressed element, at a cost of the region's size times the element's width per access); a second implementation of the device language (grow the reference that runs `apply` by its construction, `crates/exec/tests/compute_copy_reference/`); the floating-point environment through SoftFloat's state, `__float128` and the remaining intrinsics; the runtime debts below; Grover over a SQL query written in plain C; the optimiser and cost model (`opt`, not started).
6. The next independent testing pass starts at the first commit in "Which commits no independent check has covered".

## What is refused, and what removes it

| Refused today | What removes it |
|---|---|
| Emitted `apply` using recursive calls or more than 16,384 copied slots and program points | Shared call frames and return positions |
| Emitted `call_indirect` without one proved definite address | Address proofs across visits or dispatch among supported addresses |
| Emitted `apply` using loads, stack operations, register-valued call signatures, `retaddr`, remaining arithmetic or explicit instruction faults | Gate compiler routes in `docs/DESIGN.md`, "Not yet" |
| Emitted `apply` using a store | Memory in the step machine |
| Emitted `apply` using a switch without a proved definite selector | Multiway dispatch in the step machine |
| Remaining runtime operations: locks, links, directory enumeration, block transport, permissions and metadata, image loader, allocation exhaustion | The runtime debts below |
| A call through another prototype where x86-64 does not determine what is read (upper half of a 32-bit value, a missing argument that is read, an integer from a floating-point register, a variadic floating-point argument without `%al`, a narrow argument not extended as the callee relies on); `musttail` with a by-value argument | Nothing: the native result is not determined (`docs/DESIGN.md`, "Calls through a prototype") |
| A qubit-dependent call of a function that uses `setjmp`; a `setjmp` in a function with qubit variables; a function that uses `stackrestore` run coherently | The front's own effect summary of `setjmp`; resumption edges from every later call and `longjmp` in its facts (`docs/DESIGN.md`, "Non-local jumps") |
| `__float128`; `powi` on `_Float16`/`__bf16`; `#pragma STDC FENV_ACCESS`; strict (constrained) floating point; non-default rounding modes and flags; a `volatile` vector | The floating-point environment through SoftFloat's state |
| A floating-point operation that reads the subnormal mode in a function promising a non-IEEE mode (`-fdenormal-fp-math=preserve-sign`, `positive-zero`, `dynamic`, or the `-f32` forms) | The same, compared with native code run under MXCSR DAZ/FTZ |
| Stores into memory in code under a qubit that also has gates; a held object of more than 8 bytes; an access touching held bytes only in part; a position not constant outside code applied whole; an object the front cannot name (a pointer loaded from memory); qubit variables in structs, heap objects or passed to functions | Routes in `docs/DESIGN.md`, "Memory written under a qubit", and wider registers and held objects below |
| Pointers that may reach the same bytes as a held object (two objects written under a qubit that may share bytes; an ordinary access through another pointer; an escaped `restrict` copy); a call that may read or write held bytes, a callee that measures them included; a second shadow of bytes another write under a qubit still holds | A check of the addresses at run time choosing one shadow or two; ending the span before a call that only measures; one shadow from the earliest write. Today: a static function whose calls pass distinct objects, or `restrict` |
| An access at an address that depends on a qubit | Next step 5 (declare the region) |
| A gate loop whose number of passes depends on a qubit, or whose passes decide on qubit-dependent values carried between them; a gate on what its condition reads | An ordinary trip count written in C, the forms for a carried value below, or one control qubit computed once and cleared by computing it again for a gate that leaves its condition unchanged (`docs/DESIGN.md`, "Code under a qubit") |
| An update of a qubit variable under a qubit-dependent condition that neither XORs into it nor becomes straight-line code | Routes in `docs/DESIGN.md`, "Code under a qubit" |
| A qubit-dependent value used by ordinary code or printed; ordinary code reading bytes that hold qubits | The programmer writes the measurement. For held bytes the front asks for `bennett_measure_bytes`. |
| Measurement under control | Channels |
| A call that fills memory named at the call, controlled by a qubit-dependent value; a qubit-dependent call whose result is unused to a function that may write memory; two such calls in one computation when one may write memory; a call that reads memory that may change before its result is used (when a write could reach it) | Nothing for the unused call (applied, it would do nothing); one register per call for two calls; compare the call's reach with each intervening write; none designed for a call that fills memory |
| Quantum operations in `main` of a program linked with musl | Classify control functions through indirect calls, or a startup that calls `main` directly |
| Non-empty inline assembly; code without source; integers wider than 128 bits | An emulator of the real machine, compiled as an ordinary program; a type legaliser for wide integers |

## Debts

One line each, with the route:
- Dropping bytes that hold qubits. Add the device instruction that drops their qubits unexamined by partial trace.
- Run-time rotation angles. Add an ordinary angle operand to the device language and accept it in the C rotation markers for the one-control-qubit Shor program.
- Wide registers and held objects. Extend the device language beyond 64 qubits per register and the front beyond 8 held bytes per object before Shor above 64 bits.
- In-place updates. Measure unfused Shor, then add the device form with a function and its inverse. Teach the optimiser to discover when it applies.
- Coherence guarantees. Keep the named shapes and their proved or tested-only status in `docs/DESIGN.md`. Make the front warn when a program uses a tested-only shape, and supply the missing proofs for gate loops, updates in loops, applied calls and overwrites that retain the old value.
- Proof paper. Mark the sections on removed machinery, settle the licence of the proof style file, then publish under `docs/`.
- Gate loops with a carried value. First build the table when the constant count gives at most 64 control bits across the gate sites. Then use one small `apply` per pass to recompute its decision for any ordinary count.
- Subnormal doubles with `printf("%a")`. Overlay musl's hexadecimal path in `fmt_fp` to match glibc's spelling, with the ignored witness in `front/tests/tests/printf_hex_float.rs`. Low priority.
- Old names. Rename the observer's API (`ApplyJournal::branches`, `BranchJournal`, `branches_total`, `branches_present`), the front's test helper `branches`, `Stats::leftover_qubits` and the wording in the experiments' READMEs, mostly in `experiments/journal-content`, when that code is next changed. Use names for runs, configurations and kept qubits.
- Unlanded branches. The advisor lists them and deletes those that only concern removed machinery.
- Large files. Split each only when it is next changed for another reason.
- A second implementation of the device language (the removed reference modelled the removed machinery; rungs 1 and 2 and several tests lost it). Route: grow the `apply` reference (small subset today: integer operations, byte loads and stores, fixed call frames, jumps, returns), instruction by instruction.
- The fallback oracle's `user_write` acceptance waits (a call that fills memory named at the call, under a qubit). Route: none designed.
- Rule A at memory, listed refusals above. Where LLVM bounds a 2- or 4-byte `memcpy` at 6, the extra bytes hold qubits with their old values. A store under a qubit faults as `qubit_memory_access` if its opening load touches bytes an earlier call left holding qubits.
- Cost of code under a qubit: each exchanged variable costs one run of the whole code (route: one register of all exchanged fields while they fit in 64 bits); a value LLVM forwards between pieces of code under a qubit, or a join the code leaves for later code, is computed again by the later `apply`; a fixed-address read is made before a region only for globals the front shows ordinary (route: the same proof for frames and heap).
- The retained-state comparison of the randomised test covers measured registers, not bytes measured at an address (route: count the cells' columns as outputs).
- Refusal messages wider than what is unsafe: a measuring callee, a second shadow and an escaped `restrict` copy; the message for a measuring callee does not say it measures.
- An overwrite of a qubit variable after its value became unknown takes the retained form where an in-place inverse would do (`if (r) y += 2`); route: invert the predicated store in place when the update is a bijection on a condition that does not read `y`.
- A `_Bool` qubit costs 8 qubits; a qubit in memory is a whole byte, 8 qubits (route: cells of single bits). The region budget counts device instructions, not a unit the source fixes. An aggregate with a pointer is returned through memory (refused under qubit control at `-O2`); a packed struct of 129 bits is an `i136` (refused).
- Floating point: the documented-half and bfloat software conversion differences have ignored witnesses (`rt/softfloat/README.md`); which NaN comes back when several operands are NaNs is unspecified; `long double` encodings no arithmetic produces are read as x87 reads them, costing about 10% more steps on the long-double tests; the grouped checks of `front/tests/tests/complex.rs` count any two NaNs as equal.
- Runtime: separate per-invocation inputs (route: a symbol/relocation map and an initial-memory loader); file operations not carried (locks, links, directory enumeration, extended attributes, `renameat`/`linkat`, file-backed mmap: each must be real state, compared with native); namespace and metadata (root ownership, fixed device identity, zero timestamps, no credentials, no descriptor limit, a removed working directory names nothing); allocation exhaustion and reclamation (stage allocations before publication, reference counts); block transport and sleeping (`nanosleep` and timers trap; define sleep against the input clock first).
- `setjmp`: a stale record can match another activation at the same depth (route: an activation counter); a partial `sigjmp_buf` copy can select the wrong continuation (ignored witness `a_partial_record_copy_preserves_the_glibc_continuation`; route: keep glibc's byte-level copy semantics in the lowered environment); equal-priority constructor order across separately compiled objects (route: lower startup per unit before linking); return-address values at the outermost frame differ from native.
- Applied calls: nothing in the front checks that calls arrive as calls (route: name an inlined callee from its debug location; mark calls in control functions `noinline` before the front's optimiser). When a program needs it, compute and clear one register per call for two calls in one computation, and extend the warning beyond direct callees.
- Smaller: a null dereference does not fault; `bennett run --arg` refuses parameters wider than 64 bits; native `-O2` and the device may disagree where C leaves a result open (`fmin` of two zeros).
- Checks: automatic changed-line mutation is Rust-only, so C++ and script changes use recorded seeded edits (route: an established C++/script mutation engine with diff selection); whole-step mutation and cold pre-push builds have no seconds-long guarantee (route: cache dependency artifacts, select proven relevant tests); `scripts/mutate-diff.py` crashes on a diff with no Rust mutants; attribute inventory covers the measured envelope only (`inreg`, `preallocated`, `inalloca` unmeasured; no AVX-512 on this CPU); `cargo clippy -p bennett-front-tests --all-targets` reports two `unwrap` uses in `front/tests/tests/complex.rs`; an unused import warning in `front/tests/tests/thesis.rs`.
- A passing suite is evidence, not proof (blind spots found by mutation checks more than once); the numbers in `docs/AUDITION.md` are each proposal's own and were not recomputed.

## Questions for the maintainer

Beyond the three working notes below, there are no open questions.

- **Single-loop proof.** Taken: recognise one conditional test and straight continuing and
  exiting routes with provably lowerable integer operations. Recommend broadening the proof
  to additional total control-flow shapes only when a measured program needs it.

- **Simulator entry point for emitted gates.** Taken: opt in with `--apply-circuit auto`
  or `--apply-circuit step-machine`. Recommend keeping direct evaluation as the default while its
  instruction coverage and strict fault diagnostics exceed the gate compiler's.

- **Constant operands in emitted arithmetic.** Taken: fold bits and prove a single value
  across incoming edges and visits before selecting a shift or fixed-factor multiply. A
  conditional jump that folds to a constant contributes only its selected edge. Recommend
  keeping the proof conservative until a program needs more facts about definite control.

## Which commits no independent check has covered

- `58d6e4b` (a fix: only its own tests and the 15-minute randomised runs on it have covered it). The next independent testing pass starts there.
- `5fc7386` (the new-frame change) and `90f7305`, `6155161` (test-only: the step-rule tests); all three came from `wip/sol62-stepspec`, and no independent check has covered them yet.
- Independent review covered `506788f` through `28a1c00`. Its re-check covered the five fixes
  (`4b252c8`, `204b671`, `2eef510`, `23642c2`, `d30cc93`) and confirmed a test for each that failed before the fix.
- The re-check found that the operand check could refuse a shift amount or multiplication factor that was constant for all inputs.
  `e6cc49f` restored constant folding in that check and has had its own tests and a reading of its diff, with independent review still pending.
  The two 15-minute front-end and direct-simulator differential runs on `e6cc49f` had zero failures. Seed `0x87196caa251d0f43` generated 8,358 cases, accepted 8,024, declined 334, made 50,428 comparisons and 7,053 state checks, and accepted 2,568 loop recipes. Seed `0x0b7e11a5d1ff0007` generated 8,152 cases, accepted 7,820, declined 332, made 49,064 comparisons and 6,909 state checks, and accepted 2,496 loop recipes.
- Independent checks have yet to cover the documents-only commits `babaa5c`, `fc33dc9`, `81a19f4` and the handoff updates, including this one.
  The generator commits are test-only and had their sensitivity shown on a deliberately changed scratch front.
- Everything else in code under a qubit has had an independent testing pass or the 15-minute randomised runs; the list, commit by commit, is in `docs/RECORDS.md`, part two ("The top of the old HANDOFF").

## Where the records are

- The [model paper](docs/paper/README.md) records the circuit-model proofs and the earlier `mark` and `rewind` design.
- `docs/RECORDS.md`: dated measurements, testing passes and fixes (part one); the old plans, decisions and questions (part two).
- `docs/DESIGN.md` (the model, decisions in code, tests), `docs/SYNTAX.md` (text form), `docs/MARKERS.md` (marker calls), `docs/CHECKS.md` (automatic checks), `docs/NorthStar.md` and `docs/northstar-review.md` (vision and its review), `docs/AUDITION.md` (design audition), `docs/SALVAGE.md` (what the earlier workspace offers).
- Experiments: `experiments/c-corpus` (corpora), `sqlite-on-device`, `northstar-ladder`, `journal-content`, `lua-probe`, `libc-probe`; runtimes: `rt/musl`, `rt/softfloat`, `rt/libgcc` (each with a `README.md`).
- Git keeps everything removed; `f81a70a` is the last commit with `mark` and `rewind`.
