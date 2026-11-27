# HANDOFF

Updated on 2026-11-27 with the maintainer's decisions. Earlier measurements and decisions are in `docs/RECORDS.md`.

## The front end is being rebuilt on handles (2026-11-25)

**The maintainer accepted a new core design for the C front end on 2026-11-25.** Qubits live in the device and C holds
only *handles* to them. Every quantum operation is a call to an external function: allocate, release (the partial
trace), measure, gate, rotation, and `apply`, which names a plain C function and the registers it is applied to. The
front end translates each such call from the call's run-time operands and the device's register table and from
nothing else. It never asks whether a value, a store or a statement depends on a qubit. `docs/HANDLES.md` describes the design.

**How it is built.** `main` has `front/include/bq.h`, the native stand-in and the five programs in `examples/handles` (the
thesis program, Grover over a C predicate, Shor for 15 and for 21, Grover over SQLite). The declaration check stopped after
three review rounds and is being rebuilt. The replacement checks the unit's LLVM intermediate representation against
the header before any optimiser pass, removes claims about effects from pointer calls, and refuses other globals with
a primitive's name.
After optimising that same unit, it checks only name redirection and call and declaration shape. Some device forms are
built on `wip/o15-device` and await landing. The first milestone is still ahead: the thesis program, Grover over a C
predicate and Shor for 15 must run on the device through the new path, with equal results at `-O0` and `-O2` and the
Toffoli and qubit counts the emitter already reaches. Then Grover over SQLite.

**The old layer is frozen: no more patches to code under a qubit (stopped on 2026-11-20).** The old layer is
everything the front decides by asking whether a value, a store or a statement *depends on a qubit*: conditions and
loops on a qubit in the control program, stores of qubit-dependent values into ordinary memory with the exchange the
front adds, expressions the front packages into functions it generates, and the classification of calls as applied.

Do not start, continue or land: the guards step (1c), placement P2 and P3, the register for a value with several
consumers, any further rule, allow-list entry or refusal in that part of `front/src`, or any fix for an entry under
"Known wrong results" below. Those entries stay as they are, with their ignored witnesses. The old layer stays
untouched beside the new path until the milestone programs and Grover over SQLite pass in the new form. Then it is
deleted from `main` in one commit, and this file names the last commit that contains it.

Why it was stopped: every step of 2026-11-18 and 2026-11-19 on this ground repaired the cases a review had found and the next review
found new ones. Refusals of generated programs rose from about 4% to about 11%, and thirteen known wrong results
remain. Ordering patch after patch there was a poor decision by the advisor; it should have been stopped at the
second round and taken as evidence about the design. The cause is not a list of missing cases. Whether a value depends
on a qubit is undecidable when taken by value, and when taken by the program text it is not something a C compiler
preserves: a compiler keeps what a program computes, not how it was written. A front end that rests on that question
has to defend it against every optimiser pass, one patch at a time.

The handle design never asks the question. Work outside the front end continues as before: the device program and the
executor, the gate emitter for applied functions, the OpenQASM output, the C library and floating point, and ordinary
C against native.

## State

The restart of Bennett in one workspace, grown from the thesis program outward: rotate a bit, let ordinary code depend on it,
measure. The earlier `bennett-*` repositories are a read-only archive. Vision: `docs/NorthStar.md`; working design: `docs/DESIGN.md`.
C with a few marker calls (`docs/MARKERS.md`) goes through stock `clang` 18, then `bennett-front` (C++ over LLVM 18), which writes a
*device program* (`crates/ir`: ordinary code plus quantum instructions), which the *executor* (`crates/exec`) runs. The executor is a
simulator of the model in `docs/DESIGN.md`. One wire table tracks the simulator's live qubits for both the executor and
the device interface, which has a counter and an OpenQASM 3 writer.
The gate emitter compiles applied integer functions with one step budget for the whole `apply`.
Qubit-dependent choices and loops with a proved pass count can use control gates directly. The counting device uses the emitter, and the simulator offers it through
`--apply-circuit auto` or `--apply-circuit step-machine`. The default simulator evaluates applied functions
directly. The emitter can schedule a single loop through copied call frames with the shared budget. With up to
128 bits in an integer slot it handles bitwise OR, qubit-held shift amounts, unsigned multiplication, and
signed and unsigned quotient and remainder. A reached zero divisor writes `divide_by_zero` to the fault register.
Switches can dispatch on a qubit-held selector. Copied calls accept register types in their signatures. The emitter
uses private byte versions for loads and stores, including bounded addresses held in qubits, and schedules stack
operations and guarded accesses on the same budget. Reached `fault`, `effect`, `exit` and quantum instructions set
the corresponding fault code. The remaining refusals are listed below and in `docs/DESIGN.md`, "Not yet".

What works, one line each (the commit-by-commit table is in `docs/RECORDS.md`, part two):

- The device program (one data structure, reader, writer, validator), the executor with forward and reverse (undo journal) running, `apply`, gates, qubits in memory, and the CLI (`bennett run`, `bennett check`).
- The OpenQASM writer records its own simulated outcomes. With the same seed it follows `--device simulator --apply-circuit auto`, up to rounding. Runs that exceed `--max-configurations` need `--qasm-outcomes synthetic` to use seeded synthetic bits.
- Measurement uses half-open intervals over the computed weights and skips empty intervals.
- Emitted `apply` dispatches indirect calls among matching function addresses and writes `bad_indirect` for other targets.
- A private-memory wrapper modelling the handle form of `apply` matches the register form for identity, an 8-bit predicate and Shor for 15. Shor matches the hand-built circuit at 1,552 Toffolis and peak qubits of 37 in isolation and 47 in the harness (`docs/DESIGN.md`).
- Device registers hold from 1 to 2^32 - 1 qubits. Whole-register measurements and `apply` input and result operands still fit at most 128 bits.
- The device accepts run-time angles for `rx`, `ry` and `rz`. A non-finite angle gives `bad_angle` (18).
- C front end: ordinary C including variadic calls, 128-bit integers, struct and vector values, intrinsics, computed goto, `setjmp`/`longjmp`, return addresses, constructors and destructors; Grover over a plain C predicate on 8 bits.
- Floating point: `float`, `double`, `long double` over Berkeley SoftFloat 3e (`rt/softfloat`), checked bit for bit against hardware; `_Float16`, `__bf16`, `fmal`, conversions to and from 128-bit integers; complex helpers from libgcc 13.3.0 (`rt/libgcc`).
- The floating-point environment keeps a rounding mode and exception flags for each of SSE (`float`, `double`) and x87 (`long double`). The rounding modes and five ISO C exceptions are exact. The x86 denormal-operand flag, which records subnormal inputs, is outside that guarantee.
- C library: musl 1.2.5 vendored (`rt/musl`) at the system-call boundary, with its own startup and exit, journalled files, a run-input clock, an entropy image, signals a program sends itself.
- Heap exhaustion gives `out_of_memory` (19) when the C runtime's arena cannot hold a request.
- A musl-linked program receives `argc` and `argv` from its run image. `rt/musl/link.py` sets them at link time.
- Real C programs against native, standard output and exit status (`experiments/c-corpus/README.md`): GCC torture tests 1,574 agree, 1 refused by name; c-testsuite 219 agree, 1 over the step budget; GCC `ieee/` 67 agree; none wrong or crashing.
- The independent device-language reference (`crates/exec/tests/device_reference`) compares results or named faults, steps, memory, output, live registers and held bytes, and classical frames at checkpoints. It compares reduced density matrices, which describe the state of the remaining qubits, and checks that each sampled outcome is possible and leaves the expected state.
- SQLite, unmodified, as a SQL oracle: run, reversed exactly, Grover amplitudes checked at N = 16 to 512; and applied from C to a key that depends on a qubit register (`experiments/sqlite-on-device`). Lua 5.4.9 through musl (`experiments/lua-probe`). C North Star rungs 1 to 3 pass (`experiments/northstar-ladder`).
- `apply` from C: a call whose arguments depend on a qubit, to an ordinary function, is applied whole with one budget; its writes are undone, only its result survives.
- Code under a qubit (code whose control flow or stored value depends on a qubit): code without gates writing qubit variables is one `apply` (rule B); a gate loop with an ordinary trip count stays a loop (rule C); stores into memory at a definite address keep their value in qubits at the bytes (rule A); an overwrite that cannot be erased is kept with a warning. Two default-family randomised runs against native C on `4cc6a0d` made 21,788 and 12,940 comparisons with no mismatch (`front/tests/tests/qubit_differential.rs`).
- Ten automatic checks and an opt-in pre-push hook (`docs/CHECKS.md`).
- `mark` and `rewind` are gone; `f81a70a` is the last commit that contains them.

### Known wrong results on `main`

The ignored witnesses are in `front/tests/tests/coherence_regressions.rs` unless named otherwise.

- Merging ordinary and qubit-dependent stores at clang `-O2` makes both paths leave qubits in the byte. Native C returns 2 and the device faults with `qubit_memory_access` (`a_constant_store_selected_by_a_condition_keeps_the_byte_ordinary`).
- Folding an ordinary guard into a qubit-dependent condition makes the device enter a gate loop that C skips. The device never finishes the uncounted loop and issues 2^32 passes for the counted loop (`a_folded_guard_keeps_an_endless_loop_from_running`, `a_folded_guard_keeps_a_counted_loop_from_running`).
- A8. A copy of a stack local reads its shadow before any qubit-dependent write on that path and makes the caller's ordinary read fault (`an_ordinary_copy_of_an_unwritten_local_stays_ordinary`).
- A9. A write behind an ordinary condition counts even where the condition skips it, so a later copy reads the shadow and the caller's ordinary read faults (`a_write_behind_an_ordinary_condition_in_a_qubit_arm_does_not_hide_a_later_copy`).
- A10. A write after a measurement is used to place an earlier exchange whose opening load faults on held bytes (`a_write_after_a_measurement_does_not_place_the_earlier_exchange`).
- C1. Even when every path writes, opening an exchange faults if an earlier call left the bytes holding qubits (`a_store_into_bytes_an_earlier_call_left_holding_qubits_does_not_fault`).
- Computing one value in separate generated functions for two consumers can give them different frame addresses or different bytes of a local pointer (`one_call_feeding_two_consumers_gives_one_value_in_each_shape`, frame and pointer cases).
- A delayed applied call reads a live caller's local after a second helper writes it. Native C returns 1,541 and the device returns 2,569 (`a_call_reads_its_callers_local_before_a_later_write`).
- Two consumers apply the same generated function on opposite sides of a write to a live caller's local, so the second reads the changed value. Native C returns 771 and the device returns 1,795 (`two_consumers_keep_the_value_read_before_a_caller_local_changes`).
- Under `--taint-all`, `x ^= f(q)` with a known nonzero start uses different generated functions to compute and clear the value. The return-address witness leaves 7 in the cleared word (`a_known_nonzero_xor_start_clears_a_frame_dependent_value`).
- After clang `-O2`, a write through a `restrict` parameter or a `memcpy` from a qubit variable can still make a skipped guarded read fault (`a_write_through_restrict_preserves_a_later_read_guard`, `copying_from_a_qubit_variable_preserves_a_later_read_guard`).
- R12–R15. The keeping calls miss equal choices made by a structure copy, an equal-pointer join, a store through a pointer, or a helper storing in both arms. The join is folded before classification and a later applied call runs as ordinary code (`front/tests/tests/sealed_operations.rs`).
- S1. A program function named `bennett.exchange` or `bennett.holds.<n>` is mistaken for the front's own, crashing the front or dropping the call (`front/tests/tests/sealed_operations.rs`).

## How to check it

Run each in the foreground, from the repository root (`CARGO_BUILD_JOBS=3` on the shared laptop):

- On `c63481a`, `cargo test` passed 692 tests with 0 failures and 14 ignored. The ignored tests name their reasons, including heavy reference runs, generated runs and defect witnesses. The suite needs no LLVM. This run skipped the optional OpenQASM syntax check because the pinned parser was unavailable.
- On `c63481a`, `front/test.sh` passed 456 tests with 0 failures and 58 ignored. The ignored tests name their reasons, including the fallback oracle's waiting test, the SQLite acceptance, North Star ladder runs, musl-linked checks that need `rt/musl/build.py` first, heavy runs and defect witnesses. The script requires LLVM 18, clang 18 and CMake. It also compiles the C++ header and classifies the 88 LLVM attribute kinds.
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` both passed on `c63481a`.
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

The newest decisions appear first.

- **2026-11-27, primitive declarations and calls.** Before any optimiser pass, compare the compiled unit's primitive
  declarations and direct-call attributes with a header-only baseline built with the same flags, refuse other globals
  with primitive names, and remove effect claims from pointer calls. Unused and type-only declarations that emit
  nothing are accepted. After optimisation, check name redirection and call and declaration shape. Only modules
  marked by the new driver may call primitives through pointers. Update `docs/HANDLES.md` section 4, test the
  reserved-call witnesses, and measure the cost of removing effect claims on SQLite and c-testsuite.
- **2026-11-27, device operations.** Lower each interface call to one device instruction with run-time operands,
  including gate kind, rotation axis and angle, handle, position, control list and the applied function. A quantum
  operation inside an applied function gives code 8 before any other check. Use `y` as gate 2 and OpenQASM's `p`
  for the phase rotation, which multiplies the amplitude of `1` by `exp(i angle)` and leaves `0` unchanged.
- **2026-11-27, handles and layouts.** Number `i64` handles from 1 in successful allocation order and never reuse
  them. Widths and positions are run-time values. Slices consume no handle, `qwidth` queries a register's width,
  and control lists come from memory. A control entry is 16 bytes (handle, qubit). An `apply` field is 40 bytes
  (handle, lowest qubit, width, byte offset, direction 1 for input or 2 for output). Each member is a little-endian
  `u64`, aligned to 8 bytes. Use codes 20–24 (`bad_width`, `out_of_handles`, `bad_range`, `bad_gate`,
  `bad_argument`) and the widened meanings of 9 and 10 in `docs/HANDLES.md`.
- **2026-11-27, header names and checks.** Keep `bq_` and `BQ_`, linker names equal to the C identifiers, gate
  codes 1–8, rotation codes 9–12 and guard `BENNETT_BQ_H_INCLUDED`. `BQ_IN` and `BQ_OUT` check that the slice fits
  the member named. Keep one weak definition of the scope list for the whole program and check its merge with the
  linked C library.
- **2026-11-27, named stops and fault reporting.** The only primitives for named faults are `__bennett_out_of_memory` and
  `__bennett_bad_argument`, each with its own code inside `apply`. The scope list grows until allocation fails,
  and `bad_argument` covers a scalar slice over 64 qubits and an unmatched scope pop. Leave codes 25–31 free.
  On a fault, `bq_faults_check` reports on standard output and exits with status 4. Keep that output until the
  runtime supports standard error.
- **2026-11-27, block placement and budgets.** The device places the block from its size and the live stack and
  heap. Try the highest 16-byte-aligned start at which the block ends at or below the call's stack pointer. Keep
  link-time arguments and build definitions for `BQ_BUDGET(name)`. Decide whether to pass budgets as run
  arguments when the device reports steps per `apply`.
- **2026-11-27, musl allocation.** Requests musl can reject from their arguments alone return null with the native
  error. A request the arena cannot hold gives `out_of_memory` (19). Fix the reserved-name witnesses with the new
  driver.
- **2026-11-27, floating-point environment.** SSE and x87 carry separate modes and flags. Guarantee exact rounding
  modes and all five ISO C exceptions. Carry the denormal-operand flag outside that guarantee and document its
  gaps. Refuse a strict function that holds any 16-bit floating value by type. Keep the generic `fesetenv` stop
  until each unsupported setting has a name.
- **2026-11-27, wires and OpenQASM comments.** Keep the shared wire table for simulator register allocation and
  the device interface. Address the remaining storage debts there. Use ranges in OpenQASM discard and
  `release_zero` comments so even wide registers can be recorded without listing each qubit.
- **2026-11-27, emitter proofs.** Use one value identity across all proofs, including retained and complemented
  copies. Do this before further cost reductions and keep the three review cases as regression tests.
- **2026-11-27, native comparisons.** The native stand-in compares applied functions that compute on the block and
  ordinary memory, allocate successfully, produce results independent of numeric addresses, and return without
  calling an effect, exit or interface primitive, registering a handler or using buffered output. Keep generated
  comparisons in that class and the named stop for functions outside the stand-in's table of function entries,
  including native library functions such as `free`. Use the device as the reference from the first milestone.
- **2026-11-27, handle proofs.** Publish them as a second paper after the declaration check lands and after
  re-checking the width query, phase rotation, named stops, width checks, layouts and the declaration claim under
  linker names.

- **2026-11-25, the core of the C front end.** Qubits are handles, every quantum operation is an external call, and
  `apply` names a plain C function (the section at the top). `if (q) { ... }` and `y = q + 3` leave the language: the
  programmer names every point where qubits meet ordinary code. Shorter notation may be added in two ways only: a
  library in ordinary C that calls the interface, or a form expanded at source level, before any compiler pass, by a
  rule that looks at that form alone. A store that depends on a qubit still makes the bytes qubits (2026-11-17); it now
  happens in the private memory of an applied function, not in the control program's memory.
- **2026-11-25, the old layer.** Frozen now. Deleted from `main` in one commit once the milestone programs and Grover
  over SQLite pass in the new form. The device forms that served it (bytes holding qubits, gates and measurements at a
  memory address) stay in the device language.
- **2026-11-25, the interface before anything depends on it.** One revision of the header first: a query for a
  register's width, a phase rotation (so that a routine stays right when run under a control), control scopes that
  combine with the short forms for scalar functions, and a helper for fault registers. A step budget comes from a run
  that reports the steps used, not from a guessed constant.
- **2026-11-25, choices the proofs left open.** The private memory of an `apply` is placed before the input is
  written, at the same address for every input, aligned to 16 bytes. Addresses have one concrete 64-bit layout, and
  the loader's choices are fixed by the device. The builders of the scalar forms check explicitly that a slice has at
  most 64 qubits. Every implementation of `apply` returns its scratch qubits to 0. A quantum call inside an applied
  function has the device language's fault code 8 (one numbering). The control-scope library's failures (a pop
  without a push, overflow, no storage) are named stops of the run. musl takes its declarations of the output and exit
  primitives from the header. The claim that no program is refused for how it uses the interface is verified again
  with linker names in the first step's review.
- **2026-11-25, emitter choices ratified.** The five provisional choices stand, each widened only when a measured
  program needs it: the single-loop proof recognises one conditional test with straight continuing and exiting routes;
  the simulator evaluates applied functions directly by default and uses emitted gates with `--apply-circuit`;
  constant operands are proved conservatively; copying calls stops at 16,384 slots and program points with a refusal
  by name; width analysis traces at most eight argument qubits and 2^20 attempts, then falls back to full widths.
  The two limits are derived from the program's own work count when a measured program reaches them.

- **2026-11-20, sampling.** Each interval includes its lower endpoint and excludes its upper endpoint. Empty intervals are skipped.
- **2026-11-20, return addresses.** Keep the named `retaddr` refusal in emitted `apply` until it can read both suspended and copied call frames.
- **2026-11-20, wide registers.** Allow widths from 1 to 2^32 - 1 with the existing syntax. Refuse `measure` and `apply` operands wider than 128 qubits by name.
- **2026-11-20, addresses held in qubits.** Select immutable versions of private bytes under each candidate address's control.
- **2026-11-20, store cost.** Storing a value proved definite by the emitter's constant analysis costs no gates or qubits. Missing proofs can increase cost while preserving the result.

- **2026-11-19, D1.** A gate loop under a qubit-dependent condition is issued in full. A loop that continues indefinitely once entered continues indefinitely on the device. The front warns when LLVM finds no pass count.
- **2026-11-19, D2.** The language's meaning is guaranteed for C compiled with the project's flags. The front translates other IR as supplied.
- **2026-11-19, D3.** The device may omit a computation's fault on a path where nothing consumes its result.
- **2026-11-19, D4.** A value depends on a qubit at a place in the text if some path to that place computed the value from a qubit.

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

## Recent landings

Sixty-three commits landed after `0e947c1`, through `c63481a`. The additions are in "What works" above.

Twenty commits landed after `8554590`, through `7db3a1f`.

- Emitted `apply` handles `fault`, `effect`, `exit` and attempts at quantum instructions (`2e811e1`, `32931f1`, `8c03cda`). Copied call signatures support register types (`c7f0bcc`, `13c72ac`, `391b5da`).
- Private memory covers definite addresses (`f0caa7c`, `b2b4719`, `d1a1678`, `6275b57`), guarded accesses and stack operations (`14bf86e`, `18ebe0e`), and addresses held in qubits (`ebcfac1`, `7db3a1f`). The definite-address work also covers exhausted wire numbering and stores of proved constants.
- The independent `apply` reference adds OR, shifts, division and remainder, switches, direct and indirect calls, and stack operations. It agreed with the executor in 808,128 and 777,728 generated comparisons (`672d238`, `ae54f70`).
- OpenQASM records simulated outcomes, offers synthetic outcomes on request, checks the distinction between T and T-dagger, and prints the `apply` report after publishing the file (`bc8f8e2` through `514b63a`).

Since `814b6bf`:

- Gate loops under qubit-dependent conditions run ordinary code from an allow-list. The front refuses other operations by name, including held-byte loads and divisions without a nonzero proof, and warns where LLVM finds no pass count (`0c12459` through `e495eff`).
- The front checks that an added exchange runs only on paths with a qubit-dependent store and refuses placements by name when the check fails (`023fd0b` through `55a0730`).
- The gate emitter saves decisions for a single loop through copied calls, exercised by C Collatz (`83dea21` through `b7f9c82`).
- Shor period finding for 15 and 21 from plain C uses in-place modular multiplication and matches the hand-built Toffoli and peak qubit counts (tables in `docs/DESIGN.md`, `0b36920` through `3d230dd` and `7ba1c68` through `c7d8629`).
- `bennett run --device qasm --qasm-output FILE` writes OpenQASM 3 (`59c3739` through `fa8c4fb`).
- The randomised test has four opt-in families for calls that write globals, mixed stores, guarded loads, and gate loops with divisions or reads (`bc97141` through `810b786`, `3df7752` through `fc34628`).

Since `c5bd22a`:

- The emitter supports bitwise OR in applied functions (`f4b07e1`).
- Shift amounts can be held in qubits (`b90a6c1`, with review fixes in `2da8685`).
- The emitter supports quotient and signed remainder, with a named divide-by-zero fault (`b1e545f`, `2da8685` through `ba6a1df`).
- A switch can dispatch on a selector held in qubits (`a3aa23a` through `e1d5320`).
- Rules R1, R3 and M use one generated function for computing and clearing an evaluation, require the same stack pointer on replay, and refuse moving reads of return or frame addresses that would see a generated frame (`2800123` through `d34168f`).
- Rule O classifies before the optimiser and seals classified instructions for every pass. The dropped-argument witness now agrees with native C, and the merged-call case is refused by name (`84fd8d6` through `9be4544`).
- Placement P1 starts a held-byte exchange at the store that needs it or at the entry to code under a qubit that contains the store. It refuses placement unless every ordinary path writes every byte and the paths agree where they meet (`d001914`).

## Unlanded branches

- `wip/h14-front` (`31a6968`) holds the stopped declaration check and the `bennett-cc` driver. Reuse the driver in the replacement that checks before optimisation, then review the new check.
- `wip/o15-device` (`e1324b6`) holds the phase rotation and integer handles. Drop the replayed changes already on `main`, rebase and review the handle step.
- `wip/o15-debts` (`36c5689`) holds brute-force step tests for rotations and a counter and OpenQASM writer that keep one entry per register. Re-check the fix and rebase after the phase rotation lands.
- On 2026-11-25 the other `wip/` branches from the earlier run left the branch list. The 29 whose work is on `main` were deleted. The 20
  whose work is not on `main` are kept on the remote as `refs/archive/2026-11-25/wip/<name>`
  (`git fetch origin 'refs/archive/*:refs/archive/*'`). None of them is to be landed:
  - `opus-register` (`8f16b77`), the register for a value with several consumers, is old-layer work stopped after three review rounds. It accepts `paired-read`, which main refuses, and returns 2,569 for C's 1,541.
  - `opus-rule-o` and `opus-register-draft` are superseded and retired.
  - `sol-mutants-machine` and `sol-mutants-machine-partial` build on the removed `mark`/`rewind` machinery.
  - The rest date from 2026-11-15 to 2026-11-19 and hold one to thirteen commits each that never landed.
  - `opus-device` (`fd6d937`) joined the archive on 2026-11-26. Its work landed in adapted form.

### Notes for the next placement step

A rejected rule treated a call's result as chosen by qubits alone whenever its callee depended only on its arguments. It accepted a helper that XORed a value with its own copy and always returned zero.
The proof must establish what the callee computes. Functions marked `memory(none)` can legitimately read constant tables, so those reads must stay eligible. The rule needs independent review.

## Next steps

Read the section at the top first. Nothing in this list changes the old layer. The five lines run side by side.

1. **Front end.** Resume with the declaration check before LLVM passes, then finish the translation once the device forms land.
2. **Device.** Rebase the phase rotation and handles, review the handles, then continue with `y` and the remaining forms. Re-check the counter and OpenQASM fix for landing after the phase rotation.
3. **Emitter.** Use one value identity in all proofs (including copies and complemented copies). Support the handle form of `apply` once it lands in the device.
4. **Runtime.** Use `bq.h` for the C library's primitives once the new driver's declaration rules are ready. Start sleep and timers against the run's input clock while waiting.
5. **Testing.** Independently check wide registers, the wire table, run-time angles and sampling, and extend the reference for wide registers and computed angles. Its handle forms wait for the device forms to land.

Not to be started: the former steps on ordinary guards (1c), placement P2 and P3, and the several-consumer register.

## What is refused, and what removes it

| Refused today | What removes it |
|---|---|
| Emitted `apply` using recursive calls or more than 16,384 copied slots and program points | Shared call frames and return positions |
| An indirect call in emitted `apply` whose address facts admit an impossible recursive target | Track sets of possible addresses at joins |
| `retaddr` inside emitted `apply` | Supply the suspended callers' return sites and derive return sites for copied calls |
| `setjmp` and `longjmp` inside emitted `apply` | Private memory and non-local control transfer |
| A varying-address load or store in emitted `apply` whose span cannot be proved to fit in a declared memory region | Prove the range or declare the full span |
| A whole-register measurement or an `apply` input or result wider than 128 qubits | Copy or exchange the needed qubits into a narrower register (`docs/SYNTAX.md`, "Registers are not integers") |
| Remaining runtime operations: locks, links, directory enumeration, block transport, permissions and metadata, image loader | The runtime debts below |
| A call through another prototype where x86-64 does not determine what is read (upper half of a 32-bit value, a missing argument that is read, an integer from a floating-point register, a variadic floating-point argument without `%al`, a narrow argument not extended as the callee relies on); `musttail` with a by-value argument | Nothing: the native result is not determined (`docs/DESIGN.md`, "Calls through a prototype") |
| A qubit-dependent call of a function that uses `setjmp`; a `setjmp` in a function with qubit variables; a function that uses `stackrestore` run coherently | The front's own effect summary of `setjmp`; resumption edges from every later call and `longjmp` in its facts (`docs/DESIGN.md`, "Non-local jumps") |
| `__float128`; `powi` on `_Float16`/`__bf16`; a `volatile` vector | Wider operations and vector lowering |
| A strict function holding a `_Float16` or `__bf16` value | A determined native result across optimisation levels |
| `fesetenv` with unmasked exceptions, non-extended x87 precision, flush-to-zero, denormals-are-zero or reserved MXCSR bits | Carry the supported modes and traps in the runtime's environment, and continue to refuse reserved bits |
| A floating-point operation that reads the subnormal mode in a function promising a non-IEEE mode (`-fdenormal-fp-math=preserve-sign`, `positive-zero`, `dynamic`, or the `-f32` forms) | Carry the subnormal modes in the floating-point environment and compare with native code run under MXCSR DAZ/FTZ |
| Handle programs through the unfinished `bennett-cc` path | Land the new device forms and fault codes 20 to 24, then lower the primitives |
| Stores into memory in code under a qubit that also has gates; a held object of more than 8 bytes; an access touching held bytes only in part; a position not constant outside code applied whole; an object the front cannot name (a pointer loaded from memory); qubit variables in structs, heap objects or passed to functions | Routes in `docs/DESIGN.md`, "Memory written under a qubit", and wider registers and held objects below |
| Pointers that may reach the same bytes as a held object (two objects written under a qubit that may share bytes; an ordinary access through another pointer; an escaped `restrict` copy); a call that may read or write held bytes, a callee that measures them included; a second shadow of bytes another write under a qubit still holds | A check of the addresses at run time choosing one shadow or two; ending the span before a call that only measures; one shadow from the earliest write. Today: a static function whose calls pass distinct objects, or `restrict` |
| A C memory access that the front must lower at a qubit-dependent address | Declare the region, then exchange the addressed element |
| A gate loop whose number of passes depends on a qubit, or whose passes decide on qubit-dependent values carried between them; a gate on what its condition reads | An ordinary trip count written in C, the forms for a carried value below, or one control qubit computed once and cleared by computing it again for a gate that leaves its condition unchanged (`docs/DESIGN.md`, "Code under a qubit") |
| An update of a qubit variable under a qubit-dependent condition that neither XORs into it nor becomes straight-line code | Routes in `docs/DESIGN.md`, "Code under a qubit" |
| A qubit-dependent value used by ordinary code or printed; ordinary code reading bytes that hold qubits | The programmer writes the measurement. For held bytes the front asks for `bennett_measure_bytes`. |
| Measurement under control | Channels |
| A call that fills memory named at the call, controlled by a qubit-dependent value; a qubit-dependent call whose result is unused to a function that may write memory; two such calls in one computation when one may write memory; a call that reads memory that may change before its result is used (when a write could reach it) | Nothing for the unused call (applied, it would do nothing); one register per call for two calls; compare the call's reach with each intervening write; none designed for a call that fills memory |
| An applied helper that reads its caller's return address (`moved_helper_observes_the_original_callers_return_address`) | Count call depth or make generated frames transparent to the read |
| Quantum operations in `main` of a program linked with musl | Classify control functions through indirect calls, or a startup that calls `main` directly |
| Non-empty inline assembly; code without source; integers wider than 128 bits | An emulator of the real machine, compiled as an ordinary program; a type legaliser for wide integers |

## Debts

- Call expansion has a fixed limit of 16,384 slots and program points, checked before emitting any gates for the `apply`. If a measured program reaches it, derive the limit from the program's own work count.
- In the step machine, copied call results still do not prove indirect addresses, switches or shift amounts. Memory-address and arithmetic proofs can use facts common to all normal returns.
- For saved decisions, the emitter traces at most eight argument qubits and 2^20 instruction or terminator attempts before falling back to full widths and the full remaining budget.
- The Shor benchmark checks the factor-of-two target. Add assertions for the hand-built counts themselves.
- Indirect dispatch can over-refuse a load when target order hides a return fact. Return analysis is quadratic for distinct literal targets in straight-line code and visits calls behind a literal condition it could skip.
- Loop exit replay can forget that an input copy equals its source, causing a scratch-wire refusal. An unreachable zero-exponent return can block the in-place proof.
- A call can cost extra gates when it ignores a target address proved on its route. A stored exponent can select a more expensive in-place multiplier, and an impossible switch case can add gates to the following conditional.
- A discarded call can exhaust the proof allowance before a later switch and cause a refusal for unreachable recursion. One unproved store address disables guarded-write folding for every declared byte, including disjoint regions.
- The counter and OpenQASM writer keep one entry per wire and exhaust memory at billions of qubits. Measuring a register slice (`%r[LO:HI]`) still needs a form. The brute-force step tests for computed angles and `p` await landing.
- Undo of `qfree` can regrow configuration words infallibly. The register column table keeps its capacity after a wide register is freed, and ordered-map insertion for segments or configurations can stop on a host allocation refusal. Make those reservations fallible and reclaim unused column metadata.
- The simulator's peak no longer counts a refused byte, but there is no test hook for a host allocation refusal. A device-interface release that finds any nonzero qubit can keep all the named wires and report more leftovers.
- Scaling a sampler draw can bias computed weights at the smallest floating-point scale, outside normalised simulator states.
- The native handle stand-in counts basic blocks, so device budgets still need the per-`apply` step report (D16 in `docs/HANDLES.md`). It runs on Linux x86-64, with AArch64 untested, and does not check the applied function's compiled signature.
- The handle path still needs the driver's mark enforced and the C library rebuilt with the header's effect declarations. Lowering `__bennett_bad_argument` to fault 24 and checking the merge of the scope list and fault set's weak definitions across linked units are pending.
- Gate-loop divisor proofs over-refuse an odd multiple `column * k` tested nonzero (`gate_loop_divides_by_an_odd_multiple_tested_nonzero` is ignored).
- Gate-loop divisor proofs over-refuse `1u + (31u >> n)`.
- Exchange placement over-refuses the fallback oracle's bounded `memcpy` fixture (`docs/DESIGN.md`, "Not yet").
- Exchange placement over-refuses a second skipped-write span after a full measurement.
- P1 lost eleven acceptances and can over-refuse when LLVM moves a store after a loop. See `docs/DESIGN.md`, "Not yet".
- The P1 tests `seeded_short_loops_match_native_or_are_refused_by_name` and `later_region_overwrites_an_exchanged_value_is_refused` pass even if every case is accepted and agrees with C.
- The check for a removed classified instruction can fire inside the front's scratch function `bennett.evaluate`. It refuses `predgate` and `predtoggle`.
- A copy of a known constant under `--taint-all` can make the front emit an invalid device program, with `ret i8 7` where the reader expects the bare integer.
- Concurrent front-end tests can race while building musl in the same checkout. Rerun with `front/test.sh -- --test-threads=1`.
- Dropping bytes that hold qubits. Add the device instruction that drops their qubits unexamined by partial trace.
- Run-time rotation angles. C rotation markers still need an ordinary angle operand for the one-control-qubit Shor program.
- Wide registers and held objects. The front still needs more than 8 held bytes per object before Shor above 64 bits.
- In-place updates. The emitter recognises the modular loop used by Shor. Generalise the recognition and add the device form for a function and its inverse.
- Coherence guarantees. Keep the named shapes and their proved or tested-only status in `docs/DESIGN.md`. Make the front warn when a program uses a tested-only shape, and supply the missing proofs for gate loops, updates in loops, applied calls and overwrites that retain the old value.
- Proof paper. Mark the sections on removed machinery, settle the licence of the proof style file, then publish under `docs/`.
- Gate loops with a carried value. First build the table when the constant count gives at most 64 control bits across the gate sites. Then use one small `apply` per pass to recompute its decision for any ordinary count.
- Subnormal doubles with `printf("%a")`. Overlay musl's hexadecimal path in `fmt_fp` to match glibc's spelling, with the ignored witness in `front/tests/tests/printf_hex_float.rs`. Low priority.
- Old names. Rename the observer's API (`ApplyJournal::branches`, `BranchJournal`, `branches_total`, `branches_present`), the front's test helper `branches`, `Stats::leftover_qubits` and the wording in the experiments' READMEs, mostly in `experiments/journal-content`, when that code is next changed. Use names for runs, configurations and kept qubits.
- Unlanded branches. The advisor lists them and deletes those that only concern removed machinery.
- Large files. Split each only when it is next changed for another reason.
- Extend the independent device reference to wide registers, run-time angles and the handle forms. Reversal, sampling boundaries and limits, circuit emission and device output still need their own comparisons. Rungs 1 and 2 and several tests lost the earlier reference when the machinery it modelled was removed.
- The independent step audit found two documentation gaps: `stackrestore` accepts an in-range pointer even if it was never saved, and the `apply` finite-state argument needs the bound on zero-slot call depth from `budget B`.
- The fallback oracle's `user_write` acceptance waits (a call that fills memory named at the call, under a qubit). Route: none designed.
- Rule A at memory, listed refusals above. Where LLVM bounds a 2- or 4-byte `memcpy` at 6, the extra bytes hold qubits with their old values. A store under a qubit faults as `qubit_memory_access` if its opening load touches bytes an earlier call left holding qubits.
- Cost of code under a qubit: each exchanged variable costs one run of the whole code (route: one register of all exchanged fields while they fit in 64 bits); a value LLVM forwards between pieces of code under a qubit, or a join the code leaves for later code, is computed again by the later `apply`; a fixed-address read is made before a region only for globals the front shows ordinary (route: the same proof for frames and heap).
- The retained-state comparison of the randomised test covers measured registers, not bytes measured at an address (route: count the cells' columns as outputs).
- Refusal messages wider than what is unsafe: a measuring callee, a second shadow and an escaped `restrict` copy; the message for a measuring callee does not say it measures.
- An overwrite of a qubit variable after its value became unknown takes the retained form where an in-place inverse would do (`if (r) y += 2`); route: invert the predicated store in place when the update is a bijection on a condition that does not read `y`.
- A `_Bool` qubit costs 8 qubits; a qubit in memory is a whole byte, 8 qubits (route: cells of single bits). The region budget counts device instructions, not a unit the source fixes. An aggregate with a pointer is returned through memory (refused under qubit control at `-O2`); a packed struct of 129 bits is an `i136` (refused).
- Floating point: the documented-half and bfloat software conversion differences have ignored witnesses (`rt/softfloat/README.md`); which NaN comes back when several operands are NaNs is unspecified; `long double` encodings no arithmetic produces are read as x87 reads them, costing about 10% more steps on the long-double tests; the grouped checks of `front/tests/tests/complex.rs` count any two NaNs as equal.
- Floating-point environment: `fesetenv` uses the generic `unreachable` fault for unsupported modes. The denormal-operand flag from glibc's software `fmal` is not reproduced in all cases. Inlining a non-strict 16-bit helper into a strict caller can cause a refusal at `-O2` (ignored witness in `front/tests/tests/fenv.rs`). Some corpus programs cost more steps, including `cdivchkld` (`50e3e9e`).
- A program's own declaration, alias or pointer call can make the front drop or redirect a reserved primitive. This affects heap exhaustion, output and exit. The ifunc refusal misnames its kind. Fix the ignored `out_of_memory.rs` witnesses with the declaration check and reserved addresses of the handle path.
- Runtime: separate per-invocation inputs (route: a symbol/relocation map and an initial-memory loader); file operations not carried (locks, links, directory enumeration, extended attributes, `renameat`/`linkat`, file-backed mmap: each must be real state, compared with native); namespace and metadata (root ownership, fixed device identity, zero timestamps, no credentials, no descriptor limit, a removed working directory names nothing); reclamation (stage allocations before publication, reference counts); block transport and sleeping (`nanosleep` and timers trap; define sleep against the input clock first).
- `setjmp`: a stale record can match another activation at the same depth (route: an activation counter); a partial `sigjmp_buf` copy can select the wrong continuation (ignored witness `a_partial_record_copy_preserves_the_glibc_continuation`; route: keep glibc's byte-level copy semantics in the lowered environment); equal-priority constructor order across separately compiled objects (route: lower startup per unit before linking); return-address values at the outermost frame differ from native.
- Applied calls: nothing in the front checks that calls arrive as calls (route: name an inlined callee from its debug location; mark calls in control functions `noinline` before the front's optimiser). When a program needs it, compute and clear one register per call for two calls in one computation, and extend the warning beyond direct callees.
- Smaller: a null dereference does not fault; `bennett run --arg` refuses parameters wider than 64 bits; native `-O2` and the device may disagree where C leaves a result open (`fmin` of two zeros).
- Checks: automatic changed-line mutation is Rust-only, so C++ and script changes use recorded seeded edits (route: an established C++/script mutation engine with diff selection); whole-step mutation and cold pre-push builds have no seconds-long guarantee (route: cache dependency artifacts, select proven relevant tests); attribute inventory covers the measured envelope only (`inreg`, `preallocated`, `inalloca` unmeasured; no AVX-512 on this CPU); `cargo clippy -p bennett-front-tests --all-targets` reports two `unwrap` uses in `front/tests/tests/complex.rs`; unused import warnings in `front/tests/tests/thesis.rs` and `front/tests/tests/held_regions_independent.rs`.
- `scripts/mutate-diff.py` still has gaps around non-UTF-8 output and diffs, nonstandard JSON numbers and preserving subprocess exit codes. Its tests should check that default mode executes the selected mutations.
- A passing suite is evidence, not proof (blind spots found by mutation checks more than once); the numbers in `docs/AUDITION.md` are each proposal's own and were not recomputed.

## Questions for the maintainer

None open.

## Which commits no independent check has covered

- `58d6e4b` (a fix in the frozen layer: only its own tests and the 15-minute randomised runs on it have covered it).
- `4084a08` independently checked the frame change `5fc7386` and the step-rule tests `90f7305` and `6155161`. `bfd2a0d` audited the reference commits `672d238` and `ae54f70` and added the live-memory and held-byte checks and six boundary tests.
- The device reference landed in `b4a88b1`, `8b5007d`, `6e9d11d` and `7229beb`, covering the outer language, qubits in memory, wide integers and return addresses, and non-local jumps. `af29c22` rejects all ten surviving mutations that exposed result-checking gaps. `cb22de2` received a separate review.
- The next independent testing pass covers `bcb09c0` through `60cb95b` (wide registers, storage, run-time angles and sampling). The wide-register, storage and sampler changes have step reviews and fixes. The new device reference does not yet cover wide registers or run-time angles.
- Independent reviews covered the 2026-11-20 emitter and OpenQASM changes and the new emitter steps: indirect dispatch, the handle-price loop and store work, countdown, copied-call results, remainder, guarded writes and the wide fault-register layout fix.
- The handle design and header had three review rounds each. Their final edits were read before landing. The declaration check's reviews apply to the stopped branch. The replacement still needs its own review.
- Heap exhaustion, run arguments and the floating-point environment had independent reviews. The reserved-address witnesses remain open. The final denormal-bit fixes and strict-type rule after the third floating-point review were read for landing and have had no further independent pass.
- Both 15-minute front-end differential runs on `7db3a1f` had zero failures. Seed `0x87196caa251d0f43` generated 2,973 cases, accepted 2,641, declined 332, made 16,344 comparisons and accepted 912 loop recipes. Seed `0x0b7e11a5d1ff0007` generated 2,728 cases, accepted 2,410, declined 318, made 14,724 comparisons and accepted 838 loop recipes.
- Independent review covered `506788f` through `28a1c00`. Its re-check covered the five fixes
  (`4b252c8`, `204b671`, `2eef510`, `23642c2`, `d30cc93`) and confirmed a test for each that failed before the fix.
- Independent review found no wrong values in `a800563`, `4370390`, `e34d256` or `e6cc49f`. Review and two re-checks covered `abd3c0d` through `6e5f755` and `ccb534b`.
- The two 15-minute front-end and direct-simulator differential runs on `945275c` had zero failures. Seed `0x87196caa251d0f43` generated 8,397 cases, accepted 8,061, declined 336, made 50,656 comparisons and 7,084 state checks, and accepted 2,568 loop recipes. Seed `0x0b7e11a5d1ff0007` generated 4,863 cases, accepted 4,664, declined 199, made 29,372 comparisons and 4,093 state checks, and accepted 1,488 loop recipes. The second run processed fewer cases on a shared machine.
- Independent checks have yet to cover the documents-only commits `babaa5c`, `fc33dc9`, `81a19f4`, `945275c` and the handoff updates, including this one. The emitter documents in `945275c` were reworded after an independent reading and checked against the code.
- The witness commits `9d67d30` and `96c0944` are test-only. The generator commits are also test-only and had their sensitivity shown on a deliberately changed scratch front.
- Everything else in code under a qubit has had an independent testing pass or the 15-minute randomised runs; the list, commit by commit, is in `docs/RECORDS.md`, part two ("The top of the old HANDOFF").

## Where the records are

- The [model paper](docs/paper/README.md) records the circuit-model proofs and the earlier `mark` and `rewind` design.
- `docs/RECORDS.md`: dated measurements, testing passes and fixes (part one); the old plans, decisions and questions (part two).
- `docs/DESIGN.md` (the model, decisions in code, tests), `docs/SYNTAX.md` (text form), `docs/MARKERS.md` (marker calls), `docs/CHECKS.md` (automatic checks), `docs/NorthStar.md` and `docs/northstar-review.md` (vision and its review), `docs/AUDITION.md` (design audition), `docs/SALVAGE.md` (what the earlier workspace offers).
- Experiments: `experiments/c-corpus` (corpora), `sqlite-on-device`, `northstar-ladder`, `journal-content`, `lua-probe`, `libc-probe`; runtimes: `rt/musl`, `rt/softfloat`, `rt/libgcc` (each with a `README.md`).
- Git keeps everything removed; `f81a70a` is the last commit with `mark` and `rewind`.
