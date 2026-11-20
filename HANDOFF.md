# HANDOFF

Updated on 2026-11-20 after `7db3a1f`. Earlier measurements and decisions are in `docs/RECORDS.md`.

## STOP: no more patches to code under a qubit (2026-11-20)

**The maintainer has stopped all work on the front end's handling of code under a qubit.** That is everything the
front decides by asking whether a value, a store or a statement *depends on a qubit*: conditions and loops on a qubit
in the control program, stores of qubit-dependent values into ordinary memory with the exchange the front adds,
expressions the front packages into functions it generates, and the classification of calls as applied.

Do not start, continue or land: the guards step (1c), placement P2 and P3, the fixes on `wip/opus-register`, any
further rule, allow-list entry or refusal in that part of `front/src`, or any fix for an entry under "Known wrong
results" below. Those entries stay as they are, with their ignored witnesses.

Why: every step of 2026-11-18 and 2026-11-19 on this ground repaired the cases a review had found and the next review
found new ones. Refusals of generated programs rose from about 4% to about 11%, and thirteen known wrong results
remain. Ordering patch after patch there was a poor decision by the advisor; it should have been stopped at the
second round and taken as evidence about the design. The cause is not a list of missing cases. Whether a value depends
on a qubit is undecidable when taken by value, and when taken by the program text it is not something a C compiler
preserves: a compiler keeps what a program computes, not how it was written. A front end that rests on that question
has to defend it against every optimiser pass, one patch at a time.

What happens instead: a design that never asks the question is being written and reviewed before any code changes.
Until the maintainer decides on it, work continues only on what the question does not touch: the device program and
the executor, the gate emitter for applied functions, the OpenQASM output, the C library and floating point, and
ordinary C against native. "Next steps" below is written for that.

## State

The restart of Bennett in one workspace, grown from the thesis program outward: rotate a bit, let ordinary code depend on it,
measure. The earlier `bennett-*` repositories are a read-only archive. Vision: `docs/NorthStar.md`; working design: `docs/DESIGN.md`.
C with a few marker calls (`docs/MARKERS.md`) goes through stock `clang` 18, then `bennett-front` (C++ over LLVM 18), which writes a
*device program* (`crates/ir`: ordinary code plus quantum instructions), which the *executor* (`crates/exec`) runs. The executor is a
simulator of the model in `docs/DESIGN.md`. The device interface has a counter and an OpenQASM 3 writer.
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
- C front end: ordinary C including variadic calls, 128-bit integers, struct and vector values, intrinsics, computed goto, `setjmp`/`longjmp`, return addresses, constructors and destructors; Grover over a plain C predicate on 8 bits.
- Floating point: `float`, `double`, `long double` over Berkeley SoftFloat 3e (`rt/softfloat`), checked bit for bit against hardware; `_Float16`, `__bf16`, `fmal`, conversions to and from 128-bit integers; complex helpers from libgcc 13.3.0 (`rt/libgcc`).
- C library: musl 1.2.5 vendored (`rt/musl`) at the system-call boundary, with its own startup and exit, journalled files, a run-input clock, an entropy image, signals a program sends itself.
- Real C programs against native, standard output and exit status (`experiments/c-corpus/README.md`): GCC torture tests 1,574 agree, 1 refused by name; c-testsuite 219 agree, 1 over the step budget; GCC `ieee/` 64 agree, 3 refused; none wrong or crashing.
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
- The simulator can sample an outcome of exact probability zero when a draw hits a rounded boundary. Three cancelling
  rotations with seed 0 give 1 where only 0 and 2 are possible (`crates/cli/tests/qasm_device.rs`, ignored
  `a_draw_on_a_rounded_boundary_selects_only_possible_outcomes`). The fix `fd6d937` is unlanded.

## How to check it

Run each in the foreground, from the repository root (`CARGO_BUILD_JOBS=3` on the shared laptop):

- On `7db3a1f`, `cargo test` passed 522 tests with 0 failures and 6 ignored. The ignored tests name their reasons, including heavy reference runs, generated runs and defect witnesses. The suite needs no LLVM. This run skipped the optional OpenQASM syntax check because the pinned parser was unavailable.
- On `7db3a1f`, `front/test.sh -- --test-threads=1` passed 432 tests with 0 failures and 53 ignored after the cold parallel run failed during musl header setup. The ignored tests name their reasons, including the fallback oracle's waiting test, the SQLite acceptance, North Star ladder runs, musl-linked checks that need `rt/musl/build.py` first, heavy runs and defect witnesses. The script requires LLVM 18, clang 18 and CMake. It also compiles the C++ header and classifies the 88 LLVM attribute kinds.
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` both passed on `7db3a1f`.
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

- `wip/opus-device` starts at `f0caa7c`. Its commits land in this order.
  1. Wide registers (`abaae7b`, `ee84d31`) cleared four findings. The last fix (`516f2a9`) needs a final re-check in
     round three to verify that a memory gate fault on its second byte leaves its first byte untouched. Its ignored
     regression test needs about 2.5 GB.
  2. Run-time rotation angles (`b4a2539`, `c9538f8`) were reviewed and wait for wide registers. They add `RotateBy` and fault `bad_angle` (18).
  3. The sampler fix (`fd6d937`) needs its first independent review.
- The arithmetic job's local stash `2cfafc9` dispatches indirect calls among matching addresses and writes
  `bad_indirect` for other targets. `cargo test` passed 489 tests and `front/test.sh` passed 433. A C example with two
  targets agreed on all 256 inputs with 58 Toffolis and 47 peak qubits. Push and review are pending.
- `wip/opus-register` (`8f16b77`), the register for a value with several consumers, stopped after three review rounds. It accepts `paired-read`, which main refuses, and returns 2,569 for C's 1,541. It also refuses valid cases with an unrelated write, a loop that skips a consumer, or a saved byte passed to another call.
- `wip/opus-rule-o` is superseded by the rule O commits on main and retired.
- `wip/opus-register-draft` is superseded by the later register branch and retired.
- Never to be landed: `wip/fallback-step3`, `wip/sol-mutants-machine`, `wip/sol-mutants-machine-partial` (they build on the removed `mark`/`rewind` machinery).

### Notes for the next placement step

A rejected rule treated a call's result as chosen by qubits alone whenever its callee depended only on its arguments. It accepted a helper that XORed a value with its own copy and always returned zero.
The proof must establish what the callee computes. Functions marked `memory(none)` can legitimately read constant tables, so those reads must stay eligible. The rule needs independent review.

## Next steps

Read "STOP" at the top first. Nothing in this list touches code under a qubit.

1. Re-check the wide-register fix, land wide registers and the reviewed angles in order, then review and land the sampler fix.
2. Push and review indirect dispatch from the stash.
3. Use the recorded baselines to reduce the memory costs in the gate emitter.
4. Address the runtime and floating-point debts below.

Not to be started: the former steps on ordinary guards (1c), placement P2 and P3, and the several-consumer register.
The next independent testing pass starts at the first commit in "Which commits no independent check has covered".

## What is refused, and what removes it

| Refused today | What removes it |
|---|---|
| Emitted `apply` using recursive calls or more than 16,384 copied slots and program points | Shared call frames and return positions |
| Emitted `call_indirect` without one proved definite address | Address proofs across visits or dispatch among supported addresses |
| `retaddr` inside emitted `apply` | Supply the suspended callers' return sites and derive return sites for copied calls |
| `setjmp` and `longjmp` inside emitted `apply` | Private memory and non-local control transfer |
| A varying-address load or store in emitted `apply` whose span cannot be proved to fit in a declared memory region | Prove the range or declare the full span |
| Remaining runtime operations: locks, links, directory enumeration, block transport, permissions and metadata, image loader, allocation exhaustion | The runtime debts below |
| A call through another prototype where x86-64 does not determine what is read (upper half of a 32-bit value, a missing argument that is read, an integer from a floating-point register, a variadic floating-point argument without `%al`, a narrow argument not extended as the callee relies on); `musttail` with a by-value argument | Nothing: the native result is not determined (`docs/DESIGN.md`, "Calls through a prototype") |
| A qubit-dependent call of a function that uses `setjmp`; a `setjmp` in a function with qubit variables; a function that uses `stackrestore` run coherently | The front's own effect summary of `setjmp`; resumption edges from every later call and `longjmp` in its facts (`docs/DESIGN.md`, "Non-local jumps") |
| `__float128`; `powi` on `_Float16`/`__bf16`; `#pragma STDC FENV_ACCESS`; strict (constrained) floating point; non-default rounding modes and flags; a `volatile` vector | The floating-point environment through SoftFloat's state |
| A floating-point operation that reads the subnormal mode in a function promising a non-IEEE mode (`-fdenormal-fp-math=preserve-sign`, `positive-zero`, `dynamic`, or the `-f32` forms) | The same, compared with native code run under MXCSR DAZ/FTZ |
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
- Emitter call results contribute no constant proof to the caller. Propagate proved results from copied callees to prove more indirect addresses, switches and shift amounts.
- For saved decisions, the emitter traces at most eight argument qubits and 2^20 instruction or terminator attempts before falling back to full widths and the full remaining budget.
- The Shor benchmark checks the factor-of-two target. Add assertions for the hand-built counts themselves.
- The emitter needs two qubits and four gates to store `srem i2 %x, 1`, whose constant value the arithmetic analysis cannot prove. That analysis alone also refuses the remainder as an address. Two guarded writes to one fixed byte retain one qubit and use two CNOTs. Fold writes to bytes reached only by fixed-address stores.
- A countdown whose only back edge follows `effect` uses 4,178 operations at budget 256, up from 21, because the schedule grows with the budget.
- The call planner follows an unreachable recursive call after a callee faults on register allocation. Summarise callees that cannot return while preserving the call's step cost.
- The counter and OpenQASM writer keep one entry per wire and exhaust memory at billions of qubits. Measuring a register slice (`%r[LO:HI]`) still needs a form. Extend the brute-force step test to cover the queued `RotateBy` instruction.
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
- Run-time rotation angles. The device operand awaits landing. C rotation markers still need an ordinary angle operand for the one-control-qubit Shor program.
- Wide registers and held objects. The device width change awaits landing. The front still needs more than 8 held bytes per object before Shor above 64 bits.
- In-place updates. The emitter recognises the modular loop used by Shor. Generalise the recognition and add the device form for a function and its inverse.
- Coherence guarantees. Keep the named shapes and their proved or tested-only status in `docs/DESIGN.md`. Make the front warn when a program uses a tested-only shape, and supply the missing proofs for gate loops, updates in loops, applied calls and overwrites that retain the old value.
- Proof paper. Mark the sections on removed machinery, settle the licence of the proof style file, then publish under `docs/`.
- Gate loops with a carried value. First build the table when the constant count gives at most 64 control bits across the gate sites. Then use one small `apply` per pass to recompute its decision for any ordinary count.
- Subnormal doubles with `printf("%a")`. Overlay musl's hexadecimal path in `fmt_fp` to match glibc's spelling, with the ignored witness in `front/tests/tests/printf_hex_float.rs`. Low priority.
- Old names. Rename the observer's API (`ApplyJournal::branches`, `BranchJournal`, `branches_total`, `branches_present`), the front's test helper `branches`, `Stats::leftover_qubits` and the wording in the experiments' READMEs, mostly in `experiments/journal-content`, when that code is next changed. Use names for runs, configurations and kept qubits.
- Unlanded branches. The advisor lists them and deletes those that only concern removed machinery.
- Large files. Split each only when it is next changed for another reason.
- Grow the independent `apply` reference into a second implementation of the device language. Rungs 1 and 2 and several tests lost the earlier reference when the machinery it modelled was removed.
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

- **Single-loop proof.** Taken: recognise one conditional test and straight continuing and
  exiting routes with provably lowerable integer operations. Recommend broadening the proof
  to additional total control-flow shapes only when a measured program needs it.

- **Simulator entry point for emitted gates.** Taken: opt in with `--apply-circuit auto`
  or `--apply-circuit step-machine`. Recommend keeping direct evaluation as the default while its
  instruction coverage and strict fault diagnostics exceed the gate compiler's.

- **Constant operands in emitted arithmetic.** Taken: fold bits and prove a single value
  across incoming edges and visits before using a constant shift amount or a fixed-factor multiply. A
  conditional jump that folds to a constant contributes only its selected edge. Recommend
  keeping the proof conservative until a program needs more facts about definite control.

- **Copy-expansion limit.** The emitter uses a fixed 16,384 units. Derive the limit from the program's own work count if a measured program hits it.

- **Width-analysis work limits.** The limits are fixed at eight argument qubits and 2^20 attempts. If a measured program reaches them, derive the limits from the trace's work count.

## Which commits no independent check has covered

- `58d6e4b` (a fix: only its own tests and the 15-minute randomised runs on it have covered it). The next independent testing pass starts there.
- `5fc7386` (the new-frame change) and `90f7305`, `6155161` (test-only: the step-rule tests); all three came from `wip/sol62-stepspec`, and no independent check has covered them yet.
- Independent reviews covered the 2026-11-20 emitter and OpenQASM changes. The test-only reference commits `672d238` and `ae54f70` await independent review.
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
