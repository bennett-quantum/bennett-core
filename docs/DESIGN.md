# Design note

A working note. It records what is decided for now and what is left open. Nothing here is a contract; change it when
the code teaches us something, and say why in the commit.

## Principles

1. **Centre outward.** The first program that must run end to end is the thesis itself: rotate a bit, let ordinary
   code depend on it, measure. Everything grows by making that ordinary code more real.
2. **Correct first, cheap second.** Where the toolchain does not know something it takes the expensive correct route
   and reports the cost. It never needs an exact analysis to be correct. In the end nothing is refused: coverage is
   total unless physics forbids it, and then the reason is stated. A "not handled yet" is a debt, acceptable only
   because it beats a wrong answer; misuse of a marker may be refused with a clear error.
3. **One of each.** One definition of the device program, one reader, one validator, one executor. A module that needs
   another module's internals is a sign the boundary is wrong.
4. **Look first for what would sink us.** The nightmare is to build for a long time and then find that one
   overlooked fact of physics, semantics or compiler theory invalidates the vision. Exponentials lurk everywhere.
   Each candidate for that fact gets the cheapest experiment that could expose it, as early as possible.
5. **Defence in depth.** No single check is trusted. Exhaustive basis-state checks, forward-then-back, comparison
   with a native build, and proof each catch what the others miss.
6. **Semantics and cost are separate.** What a coherent region *means* is defined on basis states. How much a
   circuit for it *costs* belongs to the optimiser, which must show it implements the same permutation.

## The device program (`ir`)

A module is a list of functions, an initial memory image and a memory size.

- **Values** are integers of any width from 1 to 128 bits. A pointer is an integer address into one flat byte memory.
  A quantum register has at most 64 qubits.
  There are no floating-point values: floating point arrives as library code.
- **Functions** have typed parameters and at most one result. Bodies are basic blocks in SSA form with block
  parameters. Every name is resolved to a dense index when the module is loaded; nothing is looked up by name at
  run time.
- **Classical instructions:** constants; `add sub mul udiv sdiv urem srem and or xor shl lshr ashr`; `icmp`;
  `select`; `zext sext trunc`; `load` and `store` at a width, little-endian; `alloca` (frame lifetime, or until a
  `stackrestore` to a stack pointer `stacksave` gave, which is how a variable-length array in a loop is freed); direct `call`;
  **`call_indirect` through a function address** and `faddr @f` to take one; `ret`; `br`, conditional `br`, `switch`;
  `fault code`; `effect` (console byte output for now); `exit v` (a terminator finishing the whole run with the
  entry-return outcome, even from a nested call); `setjmp` and the terminator `longjmp` (non-local jumps, below).
- **Every operation is total.** A shift by at least the width gives 0. Division by zero, an out-of-bounds access, a
  bad indirect target, `unreachable` and an exhausted budget are *named faults*: part of the result, never a crash.
- **Quantum instructions**, allowed in ordinary control code: `qalloc w`, `qfree`, single-qubit gates
  (`x h z s sdg t tdg rx ry rz`) with an optional list of control qubits, each qubit in a register or at a
  bit of memory ("Qubits in memory", below), `measure`, and `apply` (below).
- **History instructions:** `mark` and `rewind ... keep` (below).

There is one instruction set for every tier. A tier is a way of running code, not a different language.

## Running a program (`exec`)

- **Forward.** The executor counts one step per executed instruction and enforces the budget itself. The program
  carries no metering code.
- **Reverse.** In reversible mode each step appends to an undo journal exactly what it destroys: the old value of an
  overwritten slot, the old bytes of a store, the predecessor of a control transfer, the stack pointer. Reversal pops
  the journal. The journal is a few machine words per step and never holds a copy of the whole state. Console output
  goes to a tape and is unwound with it.
- **Why reverse exists.** Reversible code is quantum code with minor changes, so reversible execution is the route by
  which ordinary code is promoted. The undo journal is the history of Bennett's construction (compute forward
  keeping history, copy the result out, run backward to erase the history). Its size is the workspace the naive
  circuit would need, and shrinking it is the optimiser's pebbling problem. Un-running a whole program is also worth
  having for its own sake: it falls out of the same journal, and "forward then back returns exactly to the start" is
  one of the independent checks in the defence in depth.
- **Quantum state** is a sparse map from basis configuration (the values of all live registers) to a complex
  amplitude. Gates act on amplitudes. This is numerical (f64), and is evidence, not proof.
- **`measure`** samples with a seeded generator and collapses. Reversal past a measurement is refused with a named
  error.

### `apply`: a coherent region

`apply @f (args) -> out, fault [budget B]` runs ordinary function `@f` coherently. Each argument is a classical value
or a quantum register read as an integer. `out` and `fault` are quantum registers and may not appear among the
arguments.

Meaning, on each basis configuration separately: run `@f` on the argument values, on a private copy of classical
memory, for at most `B` steps.

- On success with result `r`: `out ^= r`, `fault` unchanged.
- On a fault with code `c`: `out` unchanged, `fault ^= c`.

Arguments are preserved and the private memory is discarded, so the region is a permutation of basis states and its
own inverse. An `effect` or `exit` reached inside a region is a fault (`effect_in_region` or `exit_in_region`). This is the lifted-oracle contract of the North Star
(§4.2). Since 2026-11-16 the executor computes it as mark, call, rewind-keep on worlds, faults and budget
included ("`apply` as mark, call, rewind-keep", below); branch enumeration, its earlier definition, is kept as
the comparison and gives the same result on every example and test.

Control needs no special form at this level: `if (q) y = f(3)` is a region whose argument is `q`.

**Region faults.** A fault on only some branches cannot be detected with certainty by anything inside the
program: a fault register flags it with probability equal to the weight of the faulting branches, and the other
branches go on with a wrong result, because the interference is gone. The executor sees every branch. It counts
each `apply` execution that faulted on some branch of nonzero amplitude, with the weight of those branches, in
the run statistics. By default such an `apply` stops the run with the run error `region fault`, leaving the state
as it was before the `apply`; `allow_region_faults` (`bennett run --allow-region-faults`) lets the run go on, for
programs that read their fault registers on purpose (`examples/fault.bn`). Programs from `front` are always run
strictly in the tests.

### History: `mark` and `rewind`

Built on 2026-11-15 as the first step of the next extension (below), on today's machine: one classical
state, with a superposition only over registers. Worlds that differ in classical state are the second
step ("Worlds", below).

- **`%h = mark`** sets `%h`, an `i64`, to the number of journal records up to and including the mark's
  own. A run that keeps no journal starts one at its first mark (a run that was not started reversible
  still cannot be reversed). In a coherent region the records are counted from the region's start, as on
  a journal of the region's own whose first record is the call into its function (the first `mark` of
  `@f` gives 2): a region's result depends on its arguments and memory only, never on the caller's
  history, so `apply` stays its own inverse. A rewind's handle is a mark of its own function (the
  validator's rule), so no handle of the caller's can be named in a region.
- **`rewind %h keep (v -> r, ...)`** checks, then copies, then un-runs:
  1. It faults with `bad_rewind`, changing nothing, unless `%h` names a mark of the running activation
     that is still in the history (the frame was not left, below its own depth, since the mark), no
     `measure` ran since it, and every register kept into was allocated before it (un-running a later
     allocation would discard the kept value). A register freed since faults `dangling_register`.
  2. It XORs each value, read now, into its register on every basis configuration, in order.
  3. It un-runs every step since the mark, latest first, exactly as reversal does: gates by their
     inverses, memory and stack writes, console output (un-printed), calls and returns, inner marks, and
     earlier rewinds (below). The copy comes before the un-run (compute, copy, uncompute), so un-running
     through gates conjugates it: `mark; h q; rewind keep (i1 1 -> q)` is H X H = Z on `q`.
     The keeps may change registers the region read, so the un-run can meet a state its history does
     not describe: a register allocated since the mark that is not zero in every configuration when its
     allocation is un-run (un-running it would discard those configurations, losing norm), or an inner
     rewind whose region, run again (below), faults (a `qfree` of a register that is not zero, an inner
     rewind meeting the same). "What is promised when history is left behind", rule 4, leaves the result
     of such an un-run unspecified, but every step stays a bijection: until worlds that no longer match
     their history are decided (`HANDOFF.md`, questions), the rewind runs forward again the steps it
     un-ran, XORs the keeps out, and faults with `bad_rewind`, having changed nothing. "Not zero" is the
     test `qfree` applies. Checked exhaustively against an independent two-qubit calculation
     (`crates/exec/tests/history_unrun.rs`): 1,776 programs of a temporary, up to two of ten gates between
     it and a kept qubit, an optional free, four preparations and both keep values; the rewind faults
     exactly where the calculation leaves the temporary nonzero (156), runs with the calculated state
     elsewhere (1,064; 556 fault at the free going forward), and every run reverses.
  4. The frame continues after the rewind, wherever the mark was. Every slot holds its value at the
     mark: a slot defined between the mark and the rewind reads, after it, as it did at the mark, so a
     program should not use one there. The validator does not check that.
- **Marks nest**, and a rewind un-runs every mark and rewind inside its region. Rewinding the same mark
  twice is allowed; the second un-runs the first, its keep included.
- **In a coherent region** (`apply`) a rewind runs, but may not keep: registers are quantum
  (`quantum_in_region`).
- **Undoing a rewind** (by reversal, or by an outer rewind reaching past it) runs its region again from
  the mark to the rewind, then XORs the kept values again. Its record keeps which visit of the rewind
  (in its frame, since the mark) it was, and running again stops at that visit, so a rewind reached
  again in a loop is restored at the visit being undone, not the first one that reaches its position.
  (The first fix kept the number of steps un-run; worlds that took paths of different lengths would then
  never be equal again, so the visit is kept instead.) The steps ran before from the same classical
  state, so they run again identically; budget, configuration limit and region-fault checks are not
  repeated. Under an outer rewind's keeps one may still fault (step 3), in its own world or in one
  that split off while it ran, and then is undone again: the outer rewind faults `bad_rewind` with
  every world exactly as before it (since 2026-11-16 also when the failing world split off, which
  panicked before). A `measure` reached while running again is such a fault, refused before it samples
  or observes: no rewound region held one, so it lies on a path the history does not describe.
  `crates/exec/src/atomic_rewind_tests.rs` compares the whole machine (every world's journal, slots,
  register table and kept copies, the generator, observations) before and after each way a rewind
  fails. An `apply` un-run or run again by a rewind may fault on some branches without stopping the
  run: it is a permutation either way, and the strict check is for running forward.
- **Cost.** A rewind counts as one step and `steps` never goes back, so the budget bounds a loop of
  rewinds. The work of un-running is counted apart (`Stats::rewound_steps`, `Stats::rerun_steps`). The
  known exponential is measured (`crates/exec/tests/history.rs`): a chain of `d` levels that each mark,
  work, call the next level and rewind costs 19, 55, 127, 271, ... 36,847 steps in all at `d` = 0, 1, 2,
  3, ... 10, a factor tending to 2 per level, against 21 + 20 `d` (221 at `d` = 10) when only the
  outermost level rewinds. A mark copies nothing: the work and journal bytes per iteration of a loop of
  mark, call, rewind are the same at 50 and 1,500 iterations. Rewinding one mark again un-runs the
  previous rewind of it, so it doubles the same way: a loop whose every iteration rewinds one mark made
  before the loop un-runs 3 * 2^(n-1) - 2 steps in all for n rewinds (`history_unrun.rs`); a mark made
  in each iteration, as above, keeps the work per iteration constant.
- **Every journalled step is a bijection** of (machine state, history), checked by brute force at small
  widths (`crates/exec/src/permutation_tests.rs`): every kind of step at every position of two
  programs, from every assignment of small values to the slots it reads or overwrites and to two memory
  bytes, on the history the run wrote and on garbage records below it (38,360 steps, 3,016 of them
  faulting); distinct inputs give distinct outputs, the history below is untouched, and stepping back
  gives the input exactly. A rewind is a bijection only of states its history describes, so it is
  checked from its mark: 8,192 starts, each giving the state at the mark with the handle set, the kept
  values XORed and the program counter after the rewind, all distinct, and stepping back gives the state
  at the rewind exactly.

### Worlds

Built on 2026-11-15 as the second step of the next extension: the quantum state is a superposition of
whole machine states (`crates/exec/src/worlds.rs`). A **world** is a classical machine state (memory,
frames, every slot, stack pointer, console tape, register table, journal, outcome) together with the
amplitudes of the register configurations that go with it; a basis state is a world and one
configuration. With one world the machine is exactly the machine above.

- **Reading a register.** A `call` or `call_indirect` may pass a register `qN` to a parameter `iN` of its
  width (the validator accepts nothing else new; a `qN` parameter still takes the handle). The callee
  receives the register's value as an integer: ordinary code depends on the qubits. If the world's
  configurations disagree on the value, the world first **splits** into one world per value, each with the
  configurations of that value, and each takes the call. From the first split on every world keeps a
  journal, as from a first `mark`. The journal also notes each read (which register, which parameter,
  whether it split).
- **Every step keeps what it destroys**, so two different worlds stay different, and stepping forward
  never makes them equal. A `rewind` that un-runs the read makes them the same machine state again; then
  they are one basis state and **merge**: their amplitudes add. That is interference, and it is exact
  wherever every un-run is faithful. `apply @f(args) -> out` is the idiom `%h = mark`, `%y = call @f(args)`,
  `rewind %h keep (%y -> out)` wherever no branch faults (shown below).
- **A read the state no longer matches.** Keeping a value into a register the region read as an integer
  would make the read's un-run discard information: two worlds could become one with their amplitudes
  added though they no longer belong together (the derivation in `HANDOFF.md`, question 1). The same
  happens when a kept value into another register reaches a read one through a gate between the read
  and the rewind (`x %q ctrl %o` carries a keep into `o` over to `q`). The un-run of each read checks
  that the register still holds the value read, in every configuration; if not, the rewind faults
  `bad_rewind` and changes nothing (the orchestrator's interim decision until the maintainer answers;
  rule 4 of "What is promised" makes it a warning in the end, so it is a debt). Keeps whose net change
  (their XOR, per register or memory byte) to a register read since the mark is not zero are refused
  before un-running; two equal keeps into it change nothing and are not; anything else is found while un-running,
  and the rewind runs forward again what it un-ran, as for the other mismatches above, or, if worlds
  split off during its un-run (running an earlier merging rewind again) or it un-ran past its split
  point, restores a copy of the world it took first. Below a world's split point the check is made on
  the sum of the worlds' parts (next paragraph). Summing the parts of worlds whose
  un-runs were faithful is exactly the linear map: `h q; mark; read q; rewind; h q` measures 0 every time.
- **Shared history is un-run part by part, and checked on the sum.** Below the read where a world split
  from others, its rewind un-runs its *part* of the state the worlds shared. Every undo there is linear,
  but a part need not hold what the shared history read, nor be zero where it allocated a register (or
  freed one, in steps run again): only the sum of the parts must. So below a world's split point each
  of those checks is made on the sum. The part notes the configurations that fail it, keyed by the
  classical machine state where it is made (parts of the same state add, as their worlds would if they
  were there together); un-allocating keeps a part's zero configurations (the adjoint of allocating), and
  steps run again there do not refuse a `qfree`. When no world runs, the parked worlds with the longest
  history that un-ran such parts are all the parts summed there, as a rewind joins every world at it:
  their notes are added check by check, and a sum whose weight is above the norm tolerance (1e-9) fails.
  Then each of those worlds is put back as it was before its rewind, from a copy it kept, and faults
  `bad_rewind`, changing nothing (`RunError::WorldFault` if other worlds go on and region faults are not
  allowed). A part a check left with no configuration reads 0 and is dropped once the checks pass. A
  stop before the checks are made (a budget) keeps the copies, and reversal starts from them. The norm is
  still checked where every world has stopped (a run stops with `RunError::NormLost` if the worlds'
  weights do not sum to 1 there), as a backstop. Until 2026-11-16 these checks were off below a split
  point, so a later split hid an earlier changed read (F1 of the testing pass, `worlds_independent.rs`).
  Checked against the whole-world reference and an independent dense calculation of this meaning
  (`worlds_independent.rs`, `shared_history_class`): every order of up to three events among reads
  (one through a callee whose length depends on the value), six gates (one carries a keep into the read
  register), a temporary entangled with a kept register, and a nested merging rewind, under four
  preparations and four keeps: 12,816 programs, 11,106 run to the reference's state and reverse, 1,710
  refuse (1,376 of them run in the regular tests); four events, 134,264 programs, also agree. The
  executor before this fix returned instead of refusing on 336 of the 12,816 and panicked on 15.
- **Scheduling.** Worlds run in lockstep: one tick steps every running world once, and `steps` and the
  budget count ticks (`Stats::world_steps` counts the steps of all worlds). A world that has just rewound
  **parks** while another world runs, because that world may still rewind the same mark to the same state
  (padding is invisible in meaning: a world's state does not include time). When no world runs, equal
  worlds merge and the parked worlds with the longest history go on; a world can only become equal to
  another by rewinding a mark they share, and every world still inside that mark's region has a longer
  history. Reversal undoes the worlds with the longest history first, so worlds merge where they split
  before anything they share is undone.
- **Faults in some worlds.** As for `apply`, a fault on only some worlds cannot be detected by the program
  with certainty: by default the first world to fault while others go on stops the run with
  `RunError::WorldFault`; `allow_region_faults` lets the faulting world end and the others go on.
- **Observation.** A `measure` waits until no world runs or is parked, so every world that will merge has
  merged and the worlds' weights are the probabilities of their machine states. With several worlds it is
  the exact instrument "the machine is in this world at this measurement, or not": with this world's
  weight every other world is dropped and the register is measured; otherwise this world is dropped and
  the others keep their coherence. When every world has ended and several remain, the host reading the
  result observes the machine: one world is sampled by its weight. Both count in `Stats::observations` and
  are the first warning of `HANDOFF.md` step 4 (history still entangled at a measurement or at the end).
  Reversal refuses to cross either, as it refuses a measurement.
- **Non-local jumps** are journalled per world: a world-dependent `longjmp` discards frames in that world
  only, and a rewind un-runs it like any step, so worlds that jumped differently merge again
  (`a_jump_in_one_world_is_un_run_by_the_rewind`). A jump record lives in the world's own memory.
- **Not yet decided, stopped instead.** A world reaching, by a rewind, a state another world reached by a
  rewind and already left (only possible when a world re-rewinds a mark it shares with others, at
  another world's rewind site): whether those two interfere depends on whether time is part of the state
  (`HANDOFF.md` question). The run stops with `RunError::Unaligned`.
- **Costs.** A split copies the world's frames, slots, tape, journal and register table, and of memory only
  its overlay, the bytes it wrote since worlds share the base ("Memory as an overlay", below). Every world
  from the first split on keeps a journal. The known exponential with worlds
  (`crates/exec/tests/worlds.rs`): a chain of `d` levels that each read a qubit, mark, call the next level
  and rewind costs c(d) = 2 c(d - 1) + 47 steps of all worlds, un-run and run again (22,481 at `d` = 8),
  against 48 more per level (430 at `d` = 8) when only the outermost level rewinds; two worlds throughout.
  The apply idiom in a loop on a superposed qubit costs 31 steps of work and about 135 journal bytes per
  iteration at 50 and at 1,500 iterations. A rewind taken as a step whose region holds a split read or an
  inner rewind copies its world until its checks are made (above): steps and journal bytes are
  unchanged, and that loop's test takes about a third longer (0.26 s to 0.35 s, median of 15; all of it
  the copy, measured by leaving it out). Copying only the journal above the mark would remove most of it.
- **Checked.** Every split and merge is the linear extension of bijections
  (`permutation_tests.rs`): a reading call and a merging rewind from 128 starts each (four values of two
  arguments, two memory bytes, clean and dirty history), each on all 16 basis configurations of two 2-bit
  registers (distinct results, stepped back exactly) and on their superposition, which must step to
  exactly the worlds the basis results give and step back to itself. `apply` equals the idiom on the
  thesis program and Grover (states and outcomes over 16 seeds) and on the exhaustive `apply` harness
  (108 basis states and the arguments in superposition; the text idiom has no fault register or region
  budget, which the executor's own lowering of `apply` has: "`apply` as mark, call, rewind-keep").
  Un-running through gates across worlds equals G† X G applied to each world's part, against an
  independent dense calculation, on 91 programs (75 states equal, 16 refused exactly where the
  calculation finds an unfaithful read), and the independent whole-world reference
  (`oracle_support/worlds.rs`) agrees on the same 91.

## From C to a device program (`front`)

`front` is the only module that links LLVM. It reads the bitcode stock `clang` emits, runs LLVM's own pipeline
(without vectorisers), and writes the text form; the reader and validator in `ir` check what it wrote. The marker
surface is `docs/MARKERS.md`.

- **A qubit is a variable, not a value.** A local variable whose address is passed to a marker becomes a quantum
  register as wide as the variable. There is no allocation call.
- **A store into a qubit variable applies the whole computation of the stored value into it.** `v ^= expr` is an
  `apply` into `v`; an assignment to a variable known to be clean likewise. There are no temporaries and no
  uncompute ordering; the price is recomputation when one value feeds several stores.
- **An in-place update `x = g(x, a)`**, where g is a bijection in x that the front can invert, is three
  XORs through a fresh register n: `n ^= g(x, a); x ^= g⁻¹(n, a) ^ n; n ^= x`. g is the chain of LLVM
  operations from a load of x to the store, each a bijection in its x operand: add, sub, xor, multiply
  by an odd constant, rotation, byte swap, a select whose condition does not read x, and the truncations
  and extensions across which ring operations keep their low bits (`InPlace.h`). The other operands must
  not read x. Anything else that reads x's old value is refused: it needs history (open questions).
- **Branches, joins and loops that depend on a qubit** are outlined into ordinary functions and applied. They
  may not write memory: an ordinary write under quantum control is a channel, and is refused.
- **Quantum operations under such a branch are predicated** (`Predicate.h`). On each basis configuration the
  region runs one path; its gates and stores are moved after it, in a topological order of the region,
  each acting only where its path reaches it. A gate becomes a gate controlled by "this configuration
  reaches it"; a store `x = v` becomes `x ^= (reached ? v ^ x : 0)`, or, when that still reads x, the
  straight-line `x = reached ? v : x` that the in-place update then inverts. Each condition and change is
  an ordinary function copied from the region, control flow and all, so nothing is computed on a branch
  whose path would not compute it. A condition that LLVM's simplifier proves to be a conjunction of
  single qubits becomes the gate's controls (a CNOT, a Toffoli); any other is computed into a fresh
  qubit, controls the gate, and is computed again to clear it. A gate whose condition reads the gate's
  own variable, a value read before an operation of the region changed it and needed after, an operation
  on a cycle (a loop that is not unrolled), and a measurement are refused by name.
- **No new device instruction was needed** for any of this: gates take control lists, and `apply` XORs
  into any register.
- **Every `apply` has a fault register of its own**, released when the function returns (inside a loop: right
  after the `apply`), so the codes of two faults never cancel. A fault the run was allowed to go past ends it
  with `dirty_free`.
- **Each qubit variable is Known** (it holds an ordinary value: fresh, assigned, just measured) **or Quantum.**
- **What it cannot do yet it refuses by name**, with the source location: a qubit-dependent value used by
  ordinary code, a non-XOR overwrite of a variable whose value is not known that is not an invertible
  update, reading a qubit variable's old value after it changed, qubit variables passed to functions or
  written partially, and the cases under qubit control above.
- **`--taint-all`** treats more as tainted; tests check that no result changes.

Ordinary C that the device program has no instruction for is rewritten into what it has, with LLVM's own passes
and utilities where they exist (`Abi.h`, `Normalise.h`):

- **Variadic functions** emulate the x86-64 System V convention in memory, because clang has already lowered
  `va_arg` inline against it. A variadic function takes one more parameter, an area the caller fills: a register
  save area followed by the stack arguments, each argument placed where the psABI puts it, counting the
  registers of the named arguments. Putting everything on the stack is not enough: clang passes a
  16-byte-aligned struct that fits in two registers as two plain `i64`s and reads it back at a 16-byte boundary
  from the stack. Where LLVM's backend and the psABI disagree (an `i128` when one register is left) the front
  follows the psABI, which is what the callee's `va_arg` reads. An argument aligned to more than 16 bytes (a
  struct with `aligned(32)`, a 32- or 64-byte vector) is read at the next multiple of its alignment in memory,
  not in the area, so the area is aligned to its widest argument and the argument placed at that boundary.
  A 16-byte vector takes one vector register; a wider one, passed directly under AVX, goes to the stack.
- **Calls through a prototype that differs from the callee's definition** (an unprototyped K&R call, a
  library function redeclared with another type, a cast function pointer) read what a native x86-64 call
  reads (`lowerMismatchedCalls`, before the variadic lowering). Each side's values are laid out in registers
  and 8-byte stack slots as the convention places them, with the bits each one sets as clang 18 sets and
  relies on them (measured at -O0 and -O2): a caller extends an argument narrower than 32 bits that has
  `signext`/`zeroext` to 32 bits and nothing else, so the upper half of an `int` is not set; a callee
  extends no result. The call becomes a call with the callee's own prototype that gives each parameter the
  bits of the register or slot it reads: extra arguments are dropped, a narrower read takes the low bits, a
  pointer and an integer exchange their 64 bits, a 128-bit integer is two registers, a variadic callee gets
  the caller's later arguments as its variadic ones, and an argument the callee never reads (only kept in a
  stack slot) may be missing. Where a side would read bits the other does not set (the upper half of a
  32-bit value, a missing argument that is read, a result that is not returned, a floating-point register
  for an integer, a variadic floating-point argument from a call that does not set `%al`, a value passed
  in memory against another, a result in memory on one side only), the convention determines nothing and a
  direct call is refused by name. So is a direct call whose argument is a floating-point value of the size
  of an integer parameter or the reverse: native clang -O2 passes it as a cast (InstCombine), -O0 in a
  register of the other class, and the two native builds disagree. A callee relies on the extension of a
  narrow argument (at -O2 it uses the 32-bit register as it is; at -O0 it extends again), so where the
  callee's extension differs from the caller's the 32 bits are checked when the call runs, and a value that
  is not the extension of its low bits stops with the named fault `unreachable`. A call through a pointer
  compares the pointer with each function whose address is taken and that the device call would not reach
  as the native call does: a match is called as above, or, where nothing is determined and the device call
  would run, stops with `unreachable`; any other target keeps the device's own check (`bad_indirect`). The
  comparisons cost calls through correctly typed pointers too: on the musl tests 107 to 353 steps (0.06% to
  5%) and 5% to 16% more device code (`HANDOFF.md`, "Known weak spots").
- **Aggregate results of at most 128 bits of integer and floating-point fields** (`{ i64, i64 }`, how
  clang returns a small struct in two registers; SoftFloat's `extFloat80_t`) come back as one integer, the
  fields side by side, so code under a qubit branch can call such a function. **Other aggregate and vector
  results, and vector parameters**, go through memory: a result address, or the address of a copy.
  **Vector values** are split into their elements by LLVM's Scalarizer; a bitcast between a vector and an
  integer goes through memory first.
- **Computed goto** numbers the blocks whose address is taken and makes each `indirectbr` a `switch`, as
  LLVM's IndirectBrExpand does. **Empty inline assembly** (an optimisation barrier) is removed, its outputs
  set as the operand constraints say; any other assembly is refused.
- **Intrinsics**: LLVM's IntrinsicLowering and memory-intrinsic expansion where they exist; overflow-checking
  and saturating arithmetic by their definition in twice the width; `trap` is the fault `unreachable`.
  `stacksave` and `stackrestore` are the device program's own instructions.
- **Constant expressions** over addresses are evaluated once every global is placed.
- **Floating point** (`SoftFloat.h`) is what LLVM's soft-float legalisation does for a machine without
  floating-point hardware, as a pass over LLVM IR: each `float`, `double` and `long double` (x87 80-bit)
  operation becomes a call of the compiler runtime-call interface, the libgcc names (`__adddf3`, `__ltsf2`,
  `__fixdfsi`, `__mulxf3`, `__extenddfxf2`, ...) and for the rest the C library names of LLVM's
  RuntimeLibcalls.def (`sqrt`, `fma`, `floor`, `fmin`, `sqrtl`, ...); comparisons follow
  `softenSetCCOperands`. The routines a program needs are linked from a runtime bitcode with LinkOnlyNeeded,
  renamed so that a program's own function of the same name is never redirected, and internal, so the
  optimiser inlines and folds them; then every floating-point type becomes the integer of its width
  (`x86_fp80` is `i80`), and the front checks with the data layout that no size, alignment or struct field
  offset moves. `fneg`, `fabs`, `copysign` and `is.fpclass` are bit operations; `fmuladd` is unfused, as
  x86-64 without FMA computes it natively; `fma` is fused.
- **Where C leaves a choice, the native program's** is taken: `fmin`/`fmax` are clang's x86-64 expansion
  (`isnan(x) ? y : y < x ? y : x`, so of two equal zeros the first), `fminl`/`fmaxl` are glibc's, which the
  native program calls; an out-of-range conversion to an integer goes through the integer LLVM's x86-64 back
  end converts to (SSE 32 or 64 bits, x87 16 or 64), so it gives what native code gives.
- **The default runtime** is Berkeley SoftFloat 3e with the x86 SSE specialisation (`rt/softfloat`, measured
  in `rt/softfloat/README.md`); `long double` is computed at x87's full 64-bit precision. `--float-runtime`
  swaps the runtime at link time. Its global state: the rounding mode is never written and folds to
  round-to-nearest-even; the exception flags are never read (C reads them only through `<fenv.h>`, the C
  library's), its `softfloat_raiseFlags` does nothing, and the optimiser deletes them, so the routines write
  no memory and run under qubit control. Runtime calls on constants are evaluated after the optimiser, so a
  rotation's angle may be computed.
- **Complex multiply and divide** (`Complex.h`): clang's calls of `__mulsc3`, `__divsc3`, `__muldc3`,
  `__divdc3`, `__mulxc3`, `__divxc3` go to compiler-rt's own sources (`rt/compiler-rt`, vendored unmodified,
  decided 2026-11-15), which are ordinary C on floating-point values: the helpers a program calls and does not
  define are linked from `complex-runtime.bc` before floating point is lowered, internal, and lowered with the
  program. `--complex-runtime` swaps them. `__divxc3` needs the C library's `logbl` (musl's calls `ilogbl`,
  under `FENV_ACCESS`), so it is refused with the pragma.
- **Refused by name**: `frem` and the C library's functions (`sin`, `pow`, `strtod`, and `sqrt` under the
  default `-fmath-errno`, since errno is the C library's); operations on `__float128` (moving it is fine);
  conversions to and from integers wider than 128 bits; `powi` on `_Float16` and `__bf16` (natively the C
  library's `powf`); `#pragma STDC FENV_ACCESS`.
- **Beyond SoftFloat**: conversions to and from 128-bit integers follow libgcc's routines on x86-64 step for
  step, so out of range they give what the native program gives; `fmal`, which SoftFloat lacks for the
  80-bit format, is written over SoftFloat's primitives and rounded once by its own rounding, and agrees
  with glibc's; `_Float16` and `__bf16` are computed in `float` and rounded back after each operation, as
  LLVM's x86-64 back end computes them without half-precision hardware (`rt/softfloat/README.md`). A
  runtime routine that returns an aggregate returns it as one integer, so no result comes back through
  memory.

Two things the first C programs taught:

- An unused dependence does not exist. LLVM deletes a dead value, which is the same thing as uncomputing it, so a
  dirty discard needs a *variable* that outlives the computation.
- "The operands are still there" is not enough to uncompute; they must be unchanged. A value read from a qubit
  before a gate and used after it cannot be uncomputed, and is refused.

## Non-local jumps (`setjmp`, `longjmp`)

C's `setjmp` saves "the calling environment" and `longjmp` returns to it, from any depth. On a machine with
registers that environment is the callee-saved registers, the stack pointer and the return address, which is
what musl's x86-64 assembly saves. The device has no registers: a frame's state is its slots, its position
and the stack pointer, all in the executor. So the musl assembly is not the route, and no C routine can be:
`setjmp` must capture its caller's frame, and `longjmp` must discard frames, which no ordinary device
instruction can do. The device has one instruction and one terminator for it, as it has `exit` for leaving
from any depth:

- **`%r = setjmp %buf`** writes a 32-byte jump record at `%buf` (`docs/SYNTAX.md`): the frame's depth and
  function, the position of this `setjmp`, the frame's entry stack pointer and the current stack pointer; and
  gives the `i32` 0. The record is in program memory, so a `jmp_buf` can be copied like any other bytes, and it
  fits the smallest buffer C has for this, `__builtin_setjmp`'s five words.
- **`longjmp %buf, %v`** checks the record against the live frames: a frame of the run (inside a coherent
  region: of the region) at that depth, of that function and entry stack pointer, a `setjmp` at that position,
  and a stack pointer within that frame's allocations, at or above where they stand now. Anything else is the
  named fault `bad_indirect`, never a jump somewhere else: a jump to a function that has returned (undefined in
  C) faults unless another activation of the same function stands at the same depth and stack address, which is
  where native code would land too. Then it discards the frames above, sets the stack pointer back to where the
  `setjmp` left it (freeing what `alloca` and variable-length arrays took since, as native `longjmp` does), and
  resumes after the `setjmp`, which now gives `%v`.
- **Locals** follow from this: the frame resumes with its slots as they are, and memory as it is. An object in
  memory (every `volatile` local, every global) has its latest value, as C requires; a value the optimiser
  kept in a slot has its latest value too, which is one of the values C allows for a non-volatile local changed
  between `setjmp` and `longjmp` (indeterminate in C; natively it depends on register allocation, so tests
  observe only what C defines).
- **Un-run**: `setjmp` journals the bytes it overwrote and the result slot; `longjmp` journals every
  discarded frame (its position and entry stack pointer, two words), the target's old position, the stack
  pointer, the count and the result slot. The discarded frames' slots need no journal: nothing overwrites them
  before the jump is undone (every later write to them is journalled and undone first). So un-running through
  jumps restores the state exactly, checked step by step (`crates/exec/tests/setjmp.rs`). A `rewind` un-runs
  jumps like any other step; when it checks that its mark belongs to the running activation, a jump counts as
  the returns of the frames it discarded (the journal keeps those counts beside its marks).
- **In a coherent region** a jump within the region works on every branch, the record living in the
  region's private memory; a jump to a frame outside the region is `bad_indirect` on every branch. From C,
  though, a function that uses `setjmp` is not run coherently: LLVM must treat `setjmp` as an opaque call that
  may write any memory (a second return can follow any write), so the front sees a function that writes memory
  and refuses a qubit-dependent call of it by name. A debt, with its route: the front decides what may run
  coherently from its own summary in which `setjmp` writes only its buffer and `longjmp` only reads it (the
  device region discards private writes anyway), without giving that summary to LLVM's optimiser.

The front (`Jumps.h`), after the optimiser, so that LLVM sees the C library's `returns_twice` calls as a native
compile does: calls of the undefined library functions `setjmp`, `_setjmp` become `setjmp`; `longjmp`,
`_longjmp`, `siglongjmp`, `__longjmp_chk` become `longjmp` with C's value (0 becomes 1); `sigsetjmp` and glibc's
`__sigsetjmp(buf, savemask)` become `setjmp`. The signal mask is state of the environment, so it lives in the
buffer and never in the frame's slots: after a jump the frame resumes with the slots of its latest visit of
that call, which may have been for another buffer, or with another `savemask`. Where some `savemask` is not
constant 0, every library `setjmp` and `sigsetjmp` writes, before the device `setjmp`, the buffer's flag word
(offset 64, where C libraries keep it: `savemask` not 0) and, when it is set, saves the mask into the buffer
with the C library's `__sigsetjmp_tail(buf, 0)`; every library `longjmp`, `siglongjmp` among them, restores the
mask from its own target with `__sigsetjmp_tail(buf, 1)` when that target's flag is set, and then jumps. A copied
buffer carries its mask with it, any number of buffers can share a call site, and the mask is restored before
the jump, as glibc does (musl restores it on arrival, finding the buffer through a register its assembly saved
in it). musl's own C is linked by `rt/musl/link.py`; without the library a mask-saving `sigsetjmp` is refused as
a call with no definition, and a program without one is lowered without the flag. The jump record itself is
the rest of the environment and is all in the buffer too. clang's `__builtin_setjmp`/`__builtin_longjmp`
(`llvm.eh.sjlj.setjmp`/`longjmp`) become the same instructions with the value 1; the frame address clang stores
in the buffer, `llvm.frameaddress(0)`, is the stack pointer at the function's entry (a caller's frame address is
refused by name). The address of a library `setjmp` or `longjmp`, a call through another prototype and an
`invoke` are refused by name. The fault is `bad_indirect`, which already named a control transfer through a
value that names no valid target (`call_indirect`): the 4-bit fault code space is full (`bad_rewind` took 15),
and a code of its own needs every fault register one qubit wider (a question for the maintainer,
`HANDOFF.md`).

## Return addresses (`__builtin_return_address`)

What C observes of a return address (`llvm.returnaddress`) is an opaque code address: every call from one
site gives the same one, calls from different sites different ones, and it is not null for a frame that was
called; the number itself, and anything at a level above 0, is the machine's business (natively level 1 and
above follow the frame-pointer chain: with frame pointers omitted, clang 18 at -O2 on x86-64 reads another
word or crashes, and a sibling call skips a frame). The call site lives only in the executor's frames, so no
C routine and no rewrite in the front can give it; instrumenting every call to store its site in memory would
add a memory write to each, which a coherent region refuses, and still could not give level 1. The device has
one instruction that reads the frames:

- **`%r = retaddr N`** (an `i64`; `N` a constant) gives the code address that the frame `N` levels below the
  current one returns to: the address of the caller's function in the high 32 bits and, in the low 32, the
  position after the call in that function, counting every instruction and terminator in block order
  (`docs/SYNTAX.md`). So a code address is at least 2^32: never a function's address (its index plus 1) nor,
  below 4 GiB of memory, a data address; a call through it is `bad_indirect` and a load `out_of_bounds`, as
  calling or reading a native return address is undefined. Calls from one site give one address, from two
  sites two, for the same reason natively.
- **The outermost frame** of a run, which the executor entered and no device call did, gives
  `0xFFFF_FFFF_0000_0000`, a code address outside every function (natively it is inside the C library's
  startup, not null). With musl linked, `main` is called by musl's startup and has an ordinary return address.
  **A level beyond the outermost frame** gives 0 (natively unspecified; GCC's manual allows any value once
  the top of the stack is reached).
- **Levels above 0** follow the device's frames: the device has no frame pointers to lose and no tail calls,
  so level `N` is always the `N`-th caller, where native code gives that only with frame pointers kept and
  sibling calls off (the front's tests compare against such a native build). The meaning follows the optimised
  module, as natively: LLVM may inline a function not marked `noinline`, and then its return address is its
  caller's, on the device as natively.
- **Un-run**: `retaddr` changes nothing but its result slot, journalled like any value; un-running restores
  it exactly, and running again gives the same address, since the frames are restored too.
- **In a coherent region** the region's frames sit on the run's own, so a region callee returns to its
  `apply` and a higher level reads the frames below the region. Those are the same on every branch; the
  region's own frames may differ between branches, and each branch reads its own. A region the front
  outlines (`main.apply`) is a frame the native program does not have, so the address of a call made from it
  is not the native one; it stays opaque, so nothing C defines depends on it.
- **The journal-content observer** (`crates/exec/src/measure.rs`) does not credit a `retaddr` result as
  recomputable from live state, which keeps its counts an upper bound.

The front writes `retaddr N` for every `llvm.returnaddress(N)` in the default address space.

## Decided in code

- Memory: the stack grows down from the top; data starts at address 16; address 0 is never a function (a function's
  address is its index plus 1). A null dereference does not fault yet.
- Funnel shifts and byte swaps are lowered after the front's own analyses (it inverts them), and a funnel
  shift is lowered without a shift by the full width, which is poison in LLVM: the front runs LLVM's
  simplifier on lowered code to prove things.
- The front's tests run the executor optimised (`[profile.dev.package.bennett-exec]`): they enumerate
  2^16 branches per program.
- `alloca` results are 8-byte aligned; `front` makes a wider alignment by allocating more and rounding up. A
  `byval` parameter is copied by the callee on entry. Initialisers are written byte for byte at any width.
- The front collects `llvm.global_ctors`/`llvm.global_dtors` and explicit `.init_array[.N]`/`.fini_array[.N]`
  contributions into arrays in the order a native static link of the same module gives: LLVM emits globals in
  module order, the special arrays' entries (stable-sorted by priority, clamped to 65535) into the section named
  by their priority, an `llvm.used` (`retain`) object into a section of its own and any other object into the
  section of its name; GNU ld places numbered sections by priority, ties by name, then the plain sections in input
  order. Same-priority order is unspecified in C, and `llvm-link` (like LTO) moves the special arrays first, so a
  linked module can order ties differently from separately compiled objects. References to any of
  `__init_array_start/end` or `__fini_array_start/end` that survive the front's optimiser give C startup ownership
  of both arrays: the front defines their bounds, including empty arrays, and C calls them. The optimiser sees the
  bounds as a native compile does, as undefined linker symbols; the front defines them only after it, as the
  linker does. So a reference cannot vanish because a read of the array was folded into a direct call (there is
  nothing to fold), only with code that is dead or a value that is unused. Otherwise it wraps
  `--entry` (default `main`), calls constructors first and destructors after a normal return, in reverse array
  order. Recursive entry calls call the original body. `exit` does no automatic cleanup: the C library must run
  its exit sequence first. Associated-data constructor records, `!associated` section objects, address-taken or
  over-aligned section objects/bounds, nonnumeric section suffixes, old `.ctors`/`.dtors`, `.preinit_array`, and
  `.init`/`.fini` code sections are refused by name; they need linker semantics not supplied here. A missing
  automatic entry is refused rather than dropping startup.
- `exit` takes a typed integer operand (`exit %v` or `exit i32 5`). The executor checks its type against the
  selected entry's result when the instruction executes, because entries are chosen at run time; a mismatch
  faults as `exit_type_mismatch` (14). Inside a coherent region it faults first as `exit_in_region` (13).
  Success changes only the outcome, journals the same finish record as an outer return, and leaves active
  frames in place for reversal. C's `_Noreturn void __bennett_exit(int)` lowers to it; like
  `void __bennett_effect(char)`, it is an ordinary device call, not a quantum marker.
- Fault codes fit in 4 bits; a fault register is at least that wide. All fifteen codes are used since
  `bad_rewind` (15).
- `qfree` requires the register to be zero in every configuration, otherwise the named fault `dirty_free`.
- `measure` reads a whole register. Amplitudes at or below `f64::EPSILON` are dropped.
- Bounds of the simulator (steps, number of basis configurations) are parameters of a run. Exceeding the
  configuration limit stops the run with a named error; it is not a program fault.

### Memory as an overlay

Built on 2026-11-16 as the third step of the next extension (`crates/exec/src/memory.rs`): memory is a
classical base shared by every world and, per world, an overlay of the bytes it wrote, so a split copies
what the world wrote, never the memory, and what the worlds hold grows with the bytes they wrote, not with
the memory's size.

- **The base is classical.** It is written only while one world owns it alone: with one world the
  machine writes the base as it wrote its byte array before, and keeps no overlay. From the first split
  every world reads the same base and writes its own overlay; the base does not change while it is
  shared. When one world is left (the others merged into it or were observed away) its overlay is
  folded into the base.
- **The overlay is normal**: it holds a byte exactly where the world's memory differs from the base. A
  write of the base's own byte removes the entry, so un-running a write (which writes the old byte
  back) leaves no trace. The overlay is then the set `{(a, m[a]) : m[a] != base[a]}`, a function of the
  memory `m` alone, whatever order and number of writes made it.
- **Equality.** Worlds compare memory by their overlays: equal overlays over the one base are equal
  memories and the converse holds by normality, so worlds that differ only in memory merge exactly when
  their memories are equal. The overlay is an ordered map, compared and hashed entry by entry in address
  order, so neither the comparison nor the hash depends on the order of the writes. (The executor
  asserts that the worlds it compares share one base.)
- **Meaning unchanged.** Loads, stores, jump records and every other access read the overlay first and
  the base beneath; snapshots and reports give the whole memory. With one world the machine is exactly
  the machine above.
- **Checked.** The brute-force bijection checks run with memory owned by one world and as an overlay
  over two different bases (115,080 single steps, 9,048 of them faulting; 24,576 rewinds; 384 reading
  calls and 384 merging rewinds over all 16 basis configurations and their superposition), comparing
  the representation, overlay included, and asserting normality at every state. 400 pairs of worlds
  written in random orders merge exactly when their memories are equal. The un-running-through-gates
  programs run again with each world writing memory between its read and the rewind (an address that
  depends on the value read, and the value): against the dense calculation and the whole-world
  reference, 150 equal and 32 refused exactly where the calculation says (`worlds_oracle.rs`).
- **Measured** (`crates/exec/tests/memory_overlay.rs`; eight worlds that each write `n` bytes, then split
  again): splits copy `4n` bytes of overlay and the overlays hold at most `8n` bytes, at 16 KiB, 1 MiB
  and 4 MiB of memory alike; with a full copy they copied 7 times the memory size. In a release build
  at 16 MiB of memory the run's peak resident size went from 134 MB to 19.7 MB and its time from about
  90 ms to about 7 ms (at 1 MiB: 11.5 MB to 4.4 MB, about 7 ms to 0.55 ms). Over the existing tests that
  split (worlds, `apply`, examples, the gate programs; 1,234 runs, 5,640 splits, memories of 256 bytes
  to 4 KiB), splits copied 7.5 MB of memory before and 0 bytes now: no world had written memory when it
  split. What a split still copies is its journal, which grows with the steps run (2.77 MB over the four
  second splits after 4,096 bytes written each, against 16 KiB of overlay): sharing the journal's
  common prefix between worlds, as the base is shared, is the route if it matters.


### Qubits in memory

Built on 2026-11-16 as the fourth step of the next extension (`crates/exec/src/cells.rs`): gates take
addresses, so a qubit can live at any bit of memory. Registers stay as they are, one way of naming
qubits among others; with one world and no qubit in memory the machine is exactly the machine above.

- **Text** (`docs/SYNTAX.md`, "Qubits in memory"). A gate's qubit is `%r[i]` or `[ADDR:BIT]`: bit
  `BIT % 8` of the byte at `ADDR + BIT / 8`, that is bit `BIT` of the little-endian integer at `ADDR`, as
  `load` and `store` see it; `ADDR` is an `i64` value or constant, `BIT` below 128, `[ADDR]` is bit 0.
  Registers and memory mix in one gate. A qubit outside memory is `out_of_bounds`; the same qubit twice
  is `qubit_alias` (the validator reports the same address operand and bit twice); inside a coherent
  region a gate is `quantum_in_region`, as before. `rewind %h keep (v -> [ADDR])` XORs `v` into the
  qubits from bit 0 of `ADDR` on, as many as `v` is wide.
- **What a qubit in memory is.** The memory of a basis state is the world's memory, except at the bytes
  that hold qubits, whose values are the configuration's. Such a byte is a **cell**: an 8-qubit register
  bound to the byte's address, part of the world's register table, so world equality and its hash
  include the cells. The world's memory holds 0 under a cell. The first gate at a byte that is not a cell
  **promotes** it: a register is allocated, the byte's value XORed into it and the memory byte set to 0;
  the gate's journal record keeps the address and the old byte. A byte stays a cell until the gate that
  promoted it is un-run, even where its qubits are classical again: that is how the state is held, not
  what it means, and it keeps the cells a function of the history, so worlds that are the same machine
  state hold the same cells.
- **The classical base never becomes quantum.** A promotion writes 0 where any write would go (the base
  while one world owns it, else the world's overlay, "Memory as an overlay") and puts the value into a
  register; no amplitude ever lives in the base or in an overlay. A split copies the cells with the
  register table; merging compares them.
- **Ordinary code reads qubits in memory as it reads a register.** A `load` whose bytes include cells, a
  `store` over cells (it first reads what it overwrites: that goes into the history), and a `setjmp`
  writing its record over cells split the world into one world per value of those cells where the
  configurations disagree, as a call passing a register to an integer parameter does ("Worlds"). Then a
  load gives the cells' values, and a store XORs old and new into each cell, which stays a cell. The
  journal notes each cell a step touched with the byte it holds after the step (the value read or
  written); un-running the step checks that the cell still holds it, as for a register read: a keep, or a
  gate carrying one, that changed it makes the rewind fault `bad_rewind`, changing nothing. Below a
  world's split point the check is made on the sum of the worlds' parts, as every such check is.
- **Coherent regions** see the cells' values: each branch of an `apply` runs on private memory holding
  the values its configuration gives the cells. A region cannot gate, so it never changes a cell.
- **Un-running.** A gate at addresses is undone by its inverse, then by un-promoting the bytes it
  promoted: the old byte is XORed out of the register, the register un-allocated and the byte written
  back. That is the un-run of an allocation, with its check: the register must hold the old byte in every
  configuration (on the sum of the parts below a split point), else the rewind faults `bad_rewind`, the
  interim rule for a register allocated since the mark and left nonzero. Un-running through gates at
  addresses conjugates as through register gates: `h [16]; %h = mark; h [16]; rewind %h keep (i1 1 ->
  [16]); h [16]` leaves the byte 1.
- **Keeps into memory** need every target byte to hold qubits since before the mark, else `bad_rewind`,
  as a register kept into must be allocated before the mark. Every access to such a byte since the mark
  is journalled with its value, so un-running finds a keep that changed what the region read or wrote
  (rule 4 of "What is promised"). A keep into a classical byte would need every load and store since the
  mark to journal its address to find the same (a question, `HANDOFF.md`); to keep into a byte, promote it
  first (`z [ADDR]` changes nothing else).
- **Not yet, each a debt with its route.** A `longjmp` whose record is held in cells that disagree is
  `bad_indirect`, where it would split worlds that may then fault separately (route: let the worlds of a
  split fault on their own, as other worlds do). `measure` takes a register only: a qubit in memory is
  read by a load, which splits worlds, and observed where a measurement or the end of the run observes
  the worlds (a question, `HANDOFF.md`). A cell is a whole byte, so a qubit in memory holds 8 qubits in
  the simulator (route: cells of single bits, if the count matters).
- **Channels** stay open as before: a cell is a register column of a world, so a mixture of worlds holds
  qubits in memory as it holds registers.
- **Checked.** Every step at qubits in memory is a bijection by brute force (`permutation_tests.rs`):
  gates that promote bytes at addresses that depend on an argument (in bounds, out of bounds, aliasing),
  a coherent region over superposed cells, loads, stores and a jump record over cells, on clean and dirty
  history and in the three memory modes: 8,616 single steps (552 faulting) and 12,288 rewinds from their
  mark through a region that promotes; a load and a store that split by a cell and the rewind that
  merges them, 192 starts each, every one on 16 basis states and their superposition (the linear
  extension of the basis results, stepped back exactly). `worlds_oracle.rs`'s 182 programs with each of
  q, r, o in a register or in memory (eight layouts; q read by a load where it is in memory, o kept into
  with `keep (... -> [66])`): 1,200 equal to the independent dense calculation of the register program,
  to the register program's own run and to the whole-world reference run on the register program (the
  reference does not model qubits in memory; it is compared through the meaning above, a cell's byte as a
  register's value), and 256 refused exactly where the dense calculation finds an unfaithful read; each
  equal case reverses to its entry. The worlds testing pass's F1 (a later split hiding a changed read) in
  all four placements of its two qubits faults `bad_rewind`; an un-promotion below a split is refused
  exactly when the keep is nonzero. Fourteen deliberate mutations of this code (promotion, un-promotion
  and its checks, cell reads, writes and their checks, regions, splits, deferral) are each caught but one:
  dropping cells from world equality, which equal journals already imply.
- **Measured** (`memory_qubits.rs`, exact counts). Grover over `examples/grover.bn`'s predicate in the
  history idiom with x and the kick qubit in registers or in memory (k = 1, 2, 8 iterations): the same
  state, splits, merges, worlds (16) and configurations (32); `steps` +2(k - 1) and world steps
  +32k - 2 (no `qalloc`s, a load and a truncation per oracle in each of 16 worlds); journal +30 bytes
  whatever k (two promotions instead of two `qalloc` records); qubits held 16 against 5 (two bytes). A
  gate whose bytes already hold qubits journals no words, as a register gate; one that promotes
  journals 16 bytes per byte promoted and a count. The eight layouts of one program: the same worlds and
  splits; a register moved into memory is held as a whole byte (8 qubits in place of its 1 or 2) and
  journals 15 bytes more (a promotion's 32 in place of a `qalloc` record's 17).

### `apply` as mark, call, rewind-keep

Built on 2026-11-16 as the last step of the next extension (`crates/exec/src/region.rs`): the executor
computes `apply @f(args) -> out, fault budget B` as the history idiom, with the region's rules on the call.
It stays one step of the run that executes it, journalled as one record and undone by applying it again.

- **Mark.** The world's journal position (a journal is started if the run keeps none, as a `mark` does).
  Marks inside the region count records from it ("History"), as the enumeration's journal of each
  branch does.
- **Call.** `@f` is entered on top of the caller's frames; a register argument is read as an integer, as a
  `call` reads one ("Worlds"): where the configurations disagree on the arguments' values the world splits
  into one world per value, and the reads are journalled. A frame that does not fit faults alike in every
  world, before any split.
- **Run, under the region's rules.** Each world runs `@f` to its end: a return, or a fault. An `effect`,
  `exit` or quantum instruction faults (`effect_in_region`, `exit_in_region`, `quantum_in_region`; a rewind
  in it runs but may not keep), a `longjmp` or return address sees the frames below as the same on every
  branch, and a jump record naming them faults `bad_indirect`, as before. The budget `B` counts each world's
  own steps, a world split off inside the region (a load of qubits in memory) starting from the steps it
  shares with the world it split from: exactly what one enumerated branch of each of its configurations
  takes, so the same inputs fault `budget`. Ordinary code in the region reads qubits in memory as anywhere
  else (a load splits the world); the enumeration instead wrote each configuration's values into private
  memory.
- **Rewind-keep.** Each world XORs its result into `out`, or its fault code into `fault`, then un-runs
  every step since the mark, checked as a rewind checks. Nothing in a region can change a register or a
  qubit in memory, so every check holds (the executor asserts it), and the worlds are then one machine state
  and merge. Where some world with nonzero weight faulted, the fault is counted with the weight of its
  configurations; unless region faults are allowed the worlds un-run with no keep, merge, and the run stops
  with `region fault` (the first faulting configuration's fault, in the order of the configurations as
  they were before the `apply`, and its count), the state as before the `apply`. A region's writes to the
  qubits in memory it read change its configurations until its un-run, so the faulting ones are taken
  after it.
- **Why the worlds run one after another and merge only at the end.** A region cannot gate, so nothing in
  it interferes, and merging inside it would join configurations that have spent different amounts of
  their budget (two worlds whose paths differed in length, equal again after an inner `rewind`). Kept apart
  until the un-run, every world's own count is its configurations' count. An inner rewind that un-runs past
  a split inside the region defers its checks to the sum of the parts; in a region every part holds what
  its history read, so the checks hold part by part and the executor makes them there.
- **Counts.** `steps`, `region_steps` (summed over configurations), the region-fault counts and weights,
  `rewound_steps` and `rerun_steps` (counted once per configuration, as the enumeration counted them), and
  the run's own splits, merges and worlds are those of branch enumeration; the region's own worlds are its
  cost. `Stats::region_cost` reports what computing the regions cost: runs (configurations enumerated or
  worlds), work (steps forward and back, rewound and run again), journal and overlay bytes each run held,
  bytes copied at splits.
- **The comparison.** `Executor::set_apply_mode` (or `BENNETT_APPLY=history|branches|compare` for a whole
  test suite) selects the idiom (the default), branch enumeration, or both: compare mode computes every
  `apply` both ways from the same state and stops at any difference in the result, the world's whole
  machine state and journal, its amplitudes (exactly: both only move them), its history state, the counts
  above or anything else of the run outside the world (budget, worlds and the other worlds themselves,
  peak worlds, the generator, ...). Everything but the cost report is restored before the enumeration
  runs, so neither way sees what the other left; `src/compare_tests.rs` corrupts the history path in 30
  ways and each is caught. `BENNETT_APPLY_TALLY=file` appends each comparison's costs. The journal-content observer classifies the
  enumeration's branches, so it computes `apply` by enumeration.
- **The text idiom** (`%h = mark`, `%y = call @f(args)`, `rewind %h keep (%y -> out)`) still has no fault
  register and no budget: a fault ends the world, and its steps count against the run's budget. That is the
  executor's lowering, not a form a program can write; a question for the maintainer (`HANDOFF.md`).
- **Checked.** In compare mode every `apply` of `cargo test` (107,687 executions in 46 tests, among them the
  bijection checks, the exhaustive harness, faults, budgets, effects, `exit`, jumps, return addresses, qubits
  in memory and inner rewinds; 325 of them strict region-fault stops) and of `front/test.sh` (21,866
  executions in 41 tests, 13.9 million configurations, 76 strict stops) agrees, as do the ignored heavy runs
  (110,121 more, the whole-world reference's rerun of the suite among them); both suites also pass with
  each way alone (245 and 155 tests). Regular tests: `apply.rs` (every basis state of the harness, faulting ones included, and
  the arguments in superposition, strict and allowed, reversed) and `apply_history.rs` (every entry of the
  three examples over 16 seeds, strict and allowed, reversed).
- **Measured** (`Stats::region_cost` in compare mode, summed per test; idiom against enumeration). Over
  `front/test.sh`: 5.57 million runs against 13.9 million (a world per argument value, not per
  configuration: spectator qubits and equal arguments share one), work 136 million steps against 171
  million (0.80), journal bytes 1.02 times, time 45.1 s against 30.9 s (1.46 times; the in-place update
  over 2^20 configurations of which only 4,096 argument values differ takes 1.46 s against 2.86 s, the
  controlled update whose every configuration differs 7.6 s against 4.4 s). Over `cargo test`: 0.90 times
  the runs, 0.81 the work, 1.20 the journal bytes (the reads are journalled), 2.6 times the time (small
  regions, where setting the run aside and checking the world dominate). The known exponential inside a
  region (`apply_history.rs`: a callee that cleans up at every level, or leaves its callees' history
  open, over 4 argument values and a spectator): the same work per run both ways, 14,311 steps at depth 8
  cleaning up (doubling per level: un-running an inner rewind runs its region again, by the keep's un-run
  as by the enumeration's undo) and 183 with the history open (20 more per level), the idiom in 4 worlds where
  the enumeration takes 8 runs. Memory: the worlds of a region run one at a time in the world's place, so
  a region holds two copies of its amplitudes (0.89 GB against 0.43 GB peak on the in-place updates, the
  run's other memory unchanged; whole front suite 4.25 GB peak either way, wall 91 s against 83 s on a
  loaded host; `cargo test` 19.8 s against 20.4 s).

## What is promised when history is left behind

Decided by the maintainer on 2026-11-13. Ordinary code that destroys information keeps what it destroyed as
history, and that history is entangled with the data unless the program un-runs it.

1. **Classical results are exact.** If the state was classical before, the C code runs, and you measure, you get
   what the C program says. By linearity the same holds branch by branch for a superposition measured at once.
2. **Coherence is guaranteed for a documented class of updates**: gates, `^=`, the in-place bijections in
   `docs/MARKERS.md`, and anything the program un-runs. The class may grow; it never shrinks.
3. **Everywhere else, how much coherence survives is unspecified.** The toolchain may keep more or less history
   as it sees fit. Leaving history entangled is not a bug and not refused; the toolchain warns.
4. **Un-running a region after something changed what it read or wrote** is likewise not refused. The result is
   unspecified and the toolchain warns. It is the null-pointer mistake of this setting.

Consequences: a test may assert a quantum state only for the documented class or after an un-run; an optimiser is
free to choose what history to keep (a planned tape, recomputation) without changing any promised behaviour.

## Floating point (decided 2026-11-15)

Decided by the maintainer on 2026-11-15: floating point is Berkeley SoftFloat 3e behind the compiler runtime-call
interface (the libgcc names), swappable at link time, for `float`, `double` and the x87 80-bit `long double`.
The reason: it covers `float` and the 80-bit format, it is a verified reference, and it needs no hand port. The
original Bennett.jl's branch-free routines are kept as a later, cheaper alternative for the optimiser, to be
checked bit for bit against SoftFloat. Mathematics functions (`sin`, `exp`, `pow`, ...) come from the C
library's own code; `sqrt` and `fma` from SoftFloat.

What is promised: every result that is not a NaN is bit for bit what the native x86-64 build computes; a NaN
result is a NaN. Which NaN, when several operands are NaNs, is unspecified, as in LLVM, whose optimiser commutes
operands, and on the hardware, where it depends on the instruction form. Measured against this machine's SSE
and x87 (`front/tests/tests/softfloat.rs`): no other difference in any operation or conversion, NaN bits
included, except for `long double` encodings no arithmetic produces and out-of-range conversions, which C
leaves undefined.

## The next extension: code that is not a clean function (decided; `mark`, `rewind`, worlds, the memory overlay, gates at addresses and `apply` as the idiom built)

Decided by the maintainer on 2026-11-13 after five independent proposals (`docs/AUDITION.md`). Built so far:
`mark` and `rewind ... keep` ("History: `mark` and `rewind`", above; the text form is `%h = mark`, a result
like any other), worlds, a superposition of whole machine states ("Worlds", above), and memory as an
overlay over a classical base ("Memory as an overlay", above), and gates that take addresses ("Qubits in
memory", above), and `apply` computed as mark, call, rewind-keep, shown equal to branch enumeration on every
example and test ("`apply` as mark, call, rewind-keep", above). `apply`, `qalloc` and `qfree` stay: the text
idiom has no fault register or budget, and `qalloc`/`qfree` were not re-expressed (`HANDOFF.md`).

- **One layering.** A journalled machine defines meaning and is the fallback that needs no analysis: the quantum
  state is a superposition of whole machine states, and each step keeps what it destroys as history. Two
  optimisations sit on top and must give the same promised results: memory lowered as objects whose accesses name
  the cells they may touch, and history shrunk to what cannot be recomputed (a compiler-planned tape).
- **Instructions:** `mark %h` and `rewind %h keep (src -> dst, ...)`. A mark remembers a point; rewind pops the
  history back to it while XORing chosen values out. Marks nest. Today's `apply` is then the idiom mark, call,
  rewind-keep.
- **Memory** is a per-branch overlay of writes over a classical base that never becomes quantum, so qubits scale
  with steps run and bytes written, never with memory size.
- **Gates take addresses,** so a qubit can live anywhere in memory.
- **`apply`, `qalloc` and `qfree` stay** as accepted forms until the new instructions pass every existing test.
- **Why optimising history is safe:** between a run and its un-run, any history that makes the run reversible gives
  the same final state, provided nothing in between changes what the run read or wrote.
- **The one known exponential:** cleaning up inside every call, inside callers that also clean up, doubles work per
  call level. Keeping the callees' history open until the caller rewinds avoids it, at the price of more history.

Experiments to run before and while building, each named by a proposal as what would show it wrong: every
journalled step is a permutation, on a clean and on a dirty history; `apply` equals mark, call, rewind-keep on every
example and test; how much of the journal is the same on all branches, recoverable, or irreducible; what a
superposed program counter costs under a schedule a compiler could build; un-running through gates equals the
conjugated operation, checked against an independent matrix calculation.

## Text form

Line-oriented and writable by hand, in the style of LLVM assembly. No hashes, no embedded JSON, no version
negotiation. The exact syntax is whatever the single reader in `ir` accepts; `examples/` shows it.

## Tests

- `examples/*.bn`: hand-written programs with expected results, including the thesis program and an interference
  test (apply the region twice, undo the rotation, measure 0 every time).
- **Reverse restores everything.** After forward-then-reverse the test harness compares full snapshots (memory,
  slots, stack, tape, amplitudes) that it took itself. The executor's own say-so is not the check.
- **`apply` is the permutation it claims.** For small widths, exhaustively over every argument value, every `out`
  value and every `fault` value, compare against evaluating the function directly.
- Random programs, forward and back.
- `cargo test` finishes in seconds. Throughput and journal bytes per step are *reported*, never gated on wall time.

## Not yet

The optimiser, any cost beyond step and qubit counts, certificates, target descriptions, a binary
format, coherent memory that outlives a region, quantum instructions inside an applied function, and
recomputation shared between conditions (each predicated operation computes and clears its own).

## Open questions

- **Budget unit.** Steps should be counted in something the source fixes (executed instructions of the normalised
  LLVM bitcode), so that a better lowering never changes which inputs fault. How the plugin tells the executor which
  instructions count is undecided.
- **The adjoint of a region that is not its own inverse.** Gates under qubit control and bijective in-place
  updates are done (above) without one.
- **Coherent memory across regions.**
- **Running code under a qubit with its history kept.** `apply` is the cleaned-up special case: compute, copy the
  result out, uncompute at once. The general case keeps the journal as quantum workspace and un-runs later. That is
  what an update with no known inverse (`x = x * x`), a loop under qubit control that contains gates, and a
  conditional boot need. `mark`, `rewind` and worlds join journal and superposition: ordinary code reads a
  qubit, keeps its history, and un-runs it. A store under qubit control is now a store in some worlds; the
  memory overlay makes that cheap, and the front's fallback (`HANDOFF.md` step 4) is what uses it.
- **Channels.** A dirty discard is the partial trace, measurement is described by effects, noise by channels; the
  finished Bennett covers them (maintainer, 2026-11-13). This step is unitaries and clean discard, and a dirty
  `qfree` is a fault. Nothing in the device program should make channels impossible to add. Worlds keep that
  open: a world is already a classical record with amplitudes, so a mixture is a weighted list of such
  superpositions, and the host's observation at the end is the first, simplest instrument.
- **Exact amplitudes** for the permutation tier.
- **Integers wider than 128 bits.** The device program has integers up to 128 bits (decided 2026-11-13: Rust's
  `u128` gives their semantics; splitting them in the front would need a type legaliser, which LLVM has only in
  SelectionDAG). Wider ones are refused by name.
