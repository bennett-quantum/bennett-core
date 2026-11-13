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
  `fault code`; `effect` (console byte output for now).
- **Every operation is total.** A shift by at least the width gives 0. Division by zero, an out-of-bounds access, a
  bad indirect target, `unreachable` and an exhausted budget are *named faults*: part of the result, never a crash.
- **Quantum instructions**, allowed in ordinary control code: `qalloc w`, `qfree`, single-qubit gates
  (`x h z s sdg t tdg rx ry rz`) with an optional list of control qubits, `measure`, and `apply` (below).

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
own inverse. An `effect` reached inside a region is a fault. This is the lifted-oracle contract of the North Star
(§4.2); branch enumeration is its definition here, not an optimisation.

Control needs no special form at this level: `if (q) y = f(3)` is a region whose argument is `q`.

**Region faults.** A fault on only some branches cannot be detected with certainty by anything inside the
program: a fault register flags it with probability equal to the weight of the faulting branches, and the other
branches go on with a wrong result, because the interference is gone. The executor sees every branch. It counts
each `apply` execution that faulted on some branch of nonzero amplitude, with the weight of those branches, in
the run statistics. By default such an `apply` stops the run with the run error `region fault`, leaving the state
as it was before the `apply`; `allow_region_faults` (`bennett run --allow-region-faults`) lets the run go on, for
programs that read their fault registers on purpose (`examples/fault.bn`). Programs from `front` are always run
strictly in the tests.

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
  follows the psABI, which is what the callee's `va_arg` reads.
- **Aggregate and vector results, and vector parameters**, go through memory: a result address, or the
  address of a copy. **Vector values** are split into their elements by LLVM's Scalarizer; a bitcast between
  a vector and an integer goes through memory first.
- **Computed goto** numbers the blocks whose address is taken and makes each `indirectbr` a `switch`, as
  LLVM's IndirectBrExpand does. **Empty inline assembly** (an optimisation barrier) is removed, its outputs
  set as the operand constraints say; any other assembly is refused.
- **Intrinsics**: LLVM's IntrinsicLowering and memory-intrinsic expansion where they exist; overflow-checking
  and saturating arithmetic by their definition in twice the width; `trap` is the fault `unreachable`.
  `stacksave` and `stackrestore` are the device program's own instructions.
- **Constant expressions** over addresses are evaluated once every global is placed.

Two things the first C programs taught:

- An unused dependence does not exist. LLVM deletes a dead value, which is the same thing as uncomputing it, so a
  dirty discard needs a *variable* that outlives the computation.
- "The operands are still there" is not enough to uncompute; they must be unchanged. A value read from a qubit
  before a gate and used after it cannot be uncomputed, and is refused.

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
- Fault codes fit in 4 bits; a fault register is at least that wide.
- `qfree` requires the register to be zero in every configuration, otherwise the named fault `dirty_free`.
- `measure` reads a whole register. Amplitudes at or below `f64::EPSILON` are dropped.
- Bounds of the simulator (steps, number of basis configurations) are parameters of a run. Exceeding the
  configuration limit stops the run with a named error; it is not a program fault.

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
  conditional boot need. The executor has both halves (journal, superposition) but no instruction joins them.
- **Channels.** A dirty discard is the partial trace, measurement is described by effects, noise by channels; the
  finished Bennett covers them (maintainer, 2026-11-13). This step is unitaries and clean discard, and a dirty
  `qfree` is a fault. Nothing in the device program should make channels impossible to add.
- **Exact amplitudes** for the permutation tier.
- **Integers wider than 128 bits.** The device program has integers up to 128 bits (decided 2026-11-13: Rust's
  `u128` gives their semantics; splitting them in the front would need a type legaliser, which LLVM has only in
  SelectionDAG). Wider ones are refused by name.
