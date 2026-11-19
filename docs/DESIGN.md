# Design note

This note records the current implementation, the language's guarantees and the questions
still open. Explain design changes in the commit.

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
6. **Semantics and cost are separate.** What an `apply` *means* is defined for each definite value of its
   arguments. How much a circuit for it *costs* belongs to the optimiser, which must show it implements the same
   permutation.

## The model

The [model paper](paper/README.md) gives the mathematical background and marks the earlier `mark` and `rewind` design.

The device program is ordinary code that issues the operations of the standard quantum circuit model, plus one
construction of its own, `apply`. Terms used below:

- A **qubit** is a two-level system with basis states 0 and 1; a **register** is a numbered group of qubits read
  as an integer, little-endian. A **basis configuration** is one definite value of every register. A pure
  quantum state assigns each configuration a complex **amplitude**, and the probability of a measurement outcome
  is the sum of the squared magnitudes of the amplitudes that give it. After a partial trace (`qfree`, below)
  the remaining registers are in general in a **mixture** (a density operator), which no single assignment of
  amplitudes describes; it is the partial trace of a pure state of a larger set of qubits (a
  **purification**), and that is how the simulator stores it ("Releasing a register").
- A **gate** is a unitary (a norm-preserving linear map) on the amplitudes; a gate that permutes basis
  configurations is a classical reversible gate.
- A **step** is one executed instruction or terminator of the device program. A **terminator** ends a
  block of instructions by transferring control or finishing the current call or run.
- A **named fault** is a way for ordinary code to end: a code from 1 to 31 (`crates/ir/src/fault.rs`), part of
  the result, never a crash.

The model has three parts.

1. **Ordinary code**: integer arithmetic, memory, jumps, calls, console output, stop. Every operation is a
   total function of the machine state (named values stored in **slots**, active calls stored in **frames**,
   the **stack pointer** below which the stack grows, the current position, memory and output), with
   named faults where C would be undefined. Ordinary code is defined on definite values: outside `apply` it
   holds them; under `apply` it runs for each definite value of the arguments, and a device carries all of
   those runs at once on qubits ("Why a device can carry it out", below).
2. **Quantum instructions issued by ordinary code**: `qalloc` (a fresh register, all qubits 0), gates with
   control qubits, `measure` (the outcome becomes an ordinary value), and `qfree`, which is the **partial
   trace**: the register is dropped unexamined, whatever it holds, and the other qubits are left in the state
   they have without it (a mixture, if they were entangled with it: their interference with it is gone). A
   gate or a measurement may name a qubit by an ordinary address ("Qubits in memory", a derived form).
   Ordinary code never sees a qubit: a register is not an integer (`docs/SYNTAX.md`), and a `load` or `store`
   touching a byte that holds qubits is the named fault `qubit_memory_access`.
3. **`apply @f(args) -> out, fault budget B`**, the one bridge from qubits to ordinary code: `@f` is ordinary
   code. For each definite value of the argument registers, run `@f` for exactly `B` steps (a run that has
   finished stays finished) on a private copy of memory, XOR the result into `out`, or the fault code into
   `fault` if it faulted or had not finished, and discard the private memory ("`apply`", below).

Parts 1 and 2 alone are the circuit model with classical control: a run issues allocations, gates,
measurements and partial traces, each chosen by ordinary code from the measurement outcomes before it, and its
first `N` steps, for any `N`, are equivalent to a finite quantum circuit whose measurements are deferred to its
end.
Part 3 is not covered by that equivalence; why a device can carry it out is argued separately ("`apply`").

Rotations accept any angle. Today angles are written as finite `f64` literals (`docs/SYNTAX.md`).
The device language and C markers will also accept an ordinary value computed by the control program,
as needed for the one-control-qubit Shor program. A device with a finite gate set approximates rotations.

## The device program (`ir`)

A module is a list of functions, an initial memory image and a memory size.

- **Values** are integers of any width from 1 to 128 bits. A pointer is an integer address into one flat byte memory.
  A quantum register currently holds at most 64 qubits. Wider registers and objects with more than
  8 bytes holding qubits are planned before running Shor above 64 bits.
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
- **Quantum instructions**, issued by ordinary code: `qalloc w`, `qfree`, single-qubit gates
  (`x h z s sdg t tdg rx ry rz`) with an optional list of control qubits, each qubit in a register or at a
  bit of memory ("Qubits in memory", below), `measure` of a register or of bits at an address, and `apply`
  (below).

There is one instruction set for every tier. A tier is a way of running code, not a different language.

**Derived forms.** Some instructions are not among the model's primitives but are defined by a
straightforward expansion into ordinary code; none reads or acts on a qubit:

- `faddr @f` is the constant `f + 1` (a function's address). `call_indirect` compares its target with the
  address of each function of the expected signature and makes the direct `call` that matches; any other
  value is the fault `bad_indirect`.
- `stacksave` reads the stack pointer, an ordinary part of the machine state; `stackrestore` checks the value
  against the current frame's allocations (else `bad_stack_restore`) and writes it back.
- `retaddr N` reads the call position recorded in the frame `N` levels down, as a load from a call stack kept
  in memory would ("Return addresses").
- `setjmp` stores four words of ordinary machine state (frame depth and function, position, two stack pointers)
  into memory; `longjmp` checks them against the live frames (else `bad_longjmp`) and returns through the
  frames above, as a chain of returns would ("Non-local jumps").
- `exit v` is the stop of the run from any depth, with the outcome of a return from the entry; `effect` is
  console output and `fault NAME` a stop with a named fault, both ordinary code as they are.

## Running a program (`exec`)

- **Forward.** The executor counts one step per executed instruction and terminator. The program carries no
  metering code. A run may be given a step limit (`Options::budget`, `bennett run --budget`), a parameter of the
  run like its memory size.
- **Reverse.** In the simulator’s reversible mode each step appends to an undo journal exactly what it overwrites: the old value
  of a slot, the old bytes of a store, the predecessor of a control transfer, the stack pointer. Reversal pops
  the journal. The journal is a few machine words per step and never holds a copy of the whole state. Console
  output goes to a tape and is unwound with it. Gates are undone by their inverses and an `apply` by applying
  it again; reversal past a measurement is refused with a named error.
- **Why reverse exists.** Keeping what each step overwrites makes a run on definite values reversible step by
  step. It is a cousin of the construction behind `apply`, not that construction: on qubits each step keeps the
  whole previous state, not only what it overwrote ("Why a device can carry it out", below). Un-running a whole
  program is worth having for its own sake, and "forward then back returns to the start" is one of the
  independent checks in the defence in depth: exactly in memory, slots, frames and output, and up to `f64`
  rounding in the amplitudes, since gates are undone by their inverses in floating point.
- **The simulator’s quantum state** stores basis configurations with nonzero amplitudes, using `f64`
  for each amplitude's real and imaginary parts. A stored configuration is a list of integers.
  A **column** is one position in that list holding the bits of a register. Columns for discarded
  qubits can remain alongside live columns. Tracing them out of the purification gives the live state.
- **The simulator’s `measure`** samples with a seeded generator and keeps only the configurations that agree with the outcome,
  rescaled. It names a register, or `N` bits at an address (`measure iN [ADDR]`, "Qubits in memory").

### What a device carries out

The executor's classical controller allocates zeroed wires (individually addressed qubits), issues named
gates with their target and controls, measures wires, discards them by partial trace or releases them
after clearing them to zero. It keeps track of wire identities, register handles and bytes holding qubits.
Measurement is the only operation that returns quantum data. The sparse simulator preserves its seeded
sampling of a whole set of bits in one draw. Each wire names a bit of a simulator column, so a measurement
or release can span columns or select part of one. If the discarded part of a column is nonzero in any
stored configuration, the simulator keeps it in an unnamed column. On a partial discard the remaining bits
stay live in their original column. A device can measure the individual qubits in a set.

`Executor::run_on` runs the controller on a supplied device for modules without `apply`.
`Executor::run_on_with_apply` takes a gate compiler and invokes it when the controller reaches an
`apply`. The simulator provides reversal, snapshots, configuration limits and release diagnostics.
Its default mode evaluates applied functions directly. Its gate mode runs the emitted X, CNOT and
Toffoli gates on batches of definite configurations, carrying each complex amplitude unchanged.
It verifies that scratch qubits (temporary circuit storage) are zero before installing the resulting state.
To report faulting applies and their weights, it sums the squared amplitudes of the input configurations
whose fault destination changed by XOR. Peak qubits include the circuit's private scratch
while the circuit runs.

The target is an ideal logical quantum computer with one- and two-qubit gates, Toffoli gates and on-demand
allocation and measurement. The wrapper forwards gates with zero or one control and X with two controls.
For any other gate with `k` controls, it uses `k - 1` scratch wires. A ladder of Toffolis computes the AND
of the controls into the last scratch wire. The wrapper applies the gate with that one control, reverses the
ladder and releases the scratch in zero. The named gate and its rotation angle pass through unchanged.

The benchmark is Shor's algorithm in plain C. Compare with a textbook-class hand-built circuit
using the same registers. Both Toffoli count and peak qubits must be within a factor of two.
Windowed circuits are a stretch target. Report the remaining factor against them. The long-term
optimiser goal is to discover opportunities such as reusing a qubit after measuring it and
match the textbook circuits.

A caller can define a named gate block once and call or repeat it through the device interface.
The default expands its operations. The counting device caches one summary per immutable block
definition, multiplies its gate totals by the repetition count and composes its gate layers using the
dependencies on the incoming wire layers. The lowering wrapper forwards native blocks unchanged
and reuses an expanded definition while its scratch wires remain available. Caches drop obsolete
definitions when preparing another block. Block scratch is allocated and
released inside one call, so repetition keeps the peak of a single call at the same starting allocation.

`bennett run --device count` reports gate totals, peak allocated qubits including scratch, and depth.
A gate's layer is one plus the latest layer on any of its wires. Allocation starts a fresh wire at layer
zero. Measurements contribute no gate layers. The CLI takes synthetic measurement bits from a seeded
generator (`--seed`, default 0) and prints the source and the consumed bits in order. Counts describe
the execution for those bits.
The library counter also accepts a fixed sequence or a source that always gives zero.

| Example | One-qubit gates | Two-qubit gates | Toffolis | Peak qubits | Depth |
|---|---:|---:|---:|---:|---:|
| `calls.bn` | 0 | 0 | 0 | 0 | 0 |
| `exit.bn` | 1 | 0 | 0 | 1 | 1 |
| `retaddr.bn` | 0 | 0 | 0 | 0 | 0 |
| `setjmp.bn` | 0 | 0 | 0 | 0 | 0 |
| `sum.bn --arg 10` | 0 | 0 | 0 | 0 | 0 |

These are the examples without `apply`. Each uses the default entry and seed.

The test tool in `crates/emit/tests/support/bits.rs` runs gate lists of X, CNOT and Toffoli on 64
definite inputs at a time. It checks that a circuit preserves its input register, XORs a Rust
reference result into the output register, and clears scratch. The checks exhaust small registers
and sample larger registers with a fixed seed.

#### OpenQASM output

`bennett run --device qasm --qasm-output FILE` streams the quantum commands of one execution to a
temporary file beside the requested path. The final comment gives the return and its scalar value
when present, or the fault or stop reason, along with the run status assigned to that controller
outcome. The terminal run summary follows publication and can independently affect the process exit
status. The exporter publishes the completed trace without replacing an existing file. A rejected
start or a trace write error leaves the requested path untouched. The trace has the usual permissions
for a new file in its directory, including the process umask on Unix.

As with the counter, `apply` is compiled to gates. Both devices decompose gates beyond
the native controls described above, so their one-qubit, two-qubit and Toffoli gate totals agree.
The file expands gate blocks at each call. Gates use `stdgates.inc` with `ctrl @` for each control,
in control-then-target order.
The rotation literals round-trip to the exact finite `f64` angles used by the run.

Each device allocation declares a fresh qubit array and resets it to zero, including allocations of
scratch. OpenQASM declarations leave the initial state undefined. The reset implements the allocation
and contributes no gates. [OpenQASM qubit declarations](https://openqasm.com/versions/3.0/language/types.html#qubits)

A `qfree` drops a register by partial trace. The file marks its qubits with a `discard` comment and
leaves them declared and idle. Ignoring those qubits gives the state of the remaining qubits after the
partial trace. The file marks the controller's requests to release wires in zero with `release_zero`
comments. Declared qubits accumulate over the run, so their total can exceed the counter's peak live qubits.

Each qubit measurement is a `measure` statement annotated with the bit the controller received.
The writer uses the counter's synthetic measurement stream (`--seed`, default 0). An annotated outcome
can have probability zero. The file follows the ordinary control decisions taken for those bits and
omits the ordinary control program. A `release_zero` request assumes a replay with the recorded outcomes.
Running the file may produce different measurements while still issuing the recorded sequence of gates.

The CLI test independently recounts gate statements and compares them with `--device count`. The
syntax test uses `python3` on `PATH` and version 1.0.1 of the reference parser. It reports a skip if
the interpreter, pinned parser or parser runtime is unavailable. Install `openqasm3[parser]==1.0.1`
in a virtual environment and put its `bin` directory on `PATH` to run the parser check.
[Python OpenQASM reference parser](https://pypi.org/project/openqasm3/1.0.1/)

### `apply`

`apply @f(args) -> out, fault budget B`. Each argument is an ordinary value or a register read as an integer;
`out` and `fault` are registers (`fault` at least 5 qubits wide) and may not appear among the arguments. The
function `@f` is the **applied function**; some fault and option names call it a *region*.

To pass bits from memory today, copy them into a register with controlled X gates, apply the
function, then undo those gates. Once C functions can take qubits through pointers, a derived
argument form will name bits at an address and expand into those steps.

`apply` must follow the rule for each instruction's step cost and effect in the [step specification](APPLY-STEP.md).

**Meaning.** For each definite value `a` of the argument registers, run `@f(a)` for exactly `B` steps on a
private copy of memory as it stands, starting from a fresh frame above the caller's:

- if it has returned `r` by then: `out ^= r`, `fault` unchanged;
- if it has stopped with a named fault `c`, or has not finished (`c` is then `budget`): `fault ^= c`, `out`
  unchanged.

Then the private memory is discarded, undoing all writes. Calls that must share their writes run
inside one applied function. The arguments are unchanged, so on the arguments and the pair
(`out`, `fault`) `apply` is the unitary that takes `|a, y, z⟩` to `|a, y ⊕ r, z⟩` for the values `a` where `@f`
returns `r`, and to `|a, y, z ⊕ c⟩` where it ends with fault code `c`: a permutation of basis
configurations, which undoes itself when applied again with the same memory and the same `B`. Inside `@f` a
quantum instruction is the fault `quantum_in_region`, `effect` is `effect_in_region` and `exit` is
`exit_in_region`, each an ordinary stop of the run of `@f`. This is the lifted-oracle contract of the North
Star (§4.2).

The executor evaluates it directly (`crates/exec/src/region.rs`); that it does is a claim
about the tree, checked by its tests ("Tests"), among them an independent reference that carries out the
construction below on small functions. For each basis configuration of nonzero amplitude it runs `@f` on
that configuration's argument values, with an undo journal of its own, until it finishes or has taken `B`
steps (the next step is the fault `budget`; a run that finished earlier would only have idled), XORs the
result or fault code into that configuration's `out` or `fault`, and undoes the run. An `apply` is one step of
the run that executes it.

Control needs no special form at this level: `if (q) y = f(3)` is an `apply` whose argument is `q`.

**One bound for the whole applied function.** `budget B` is one bound on all the steps of `@f`, however its
loops and calls nest: a step of a loop inside a loop inside a callee counts against the same `B` as any other.
There is no bound per loop. `B` is an operand, so it may be an ordinary value computed at run time; like every
ordinary value it is definite when the `apply` executes, so the circuit a device runs for this `apply` (below)
is fixed before it starts. One clock covers all the steps.

**Why a device can carry it out.** Write the state of a run of `@f` as a string of `s` bits: its slots,
frames, stack pointer, position, memory, and whether it is running, has returned a result or has stopped with
a fault code. `s` is fixed before the `apply` starts: memory has the module's size, frames and stack live in it
(a call that does not fit is the fault `stack_overflow`), and console output and `exit` fault inside, so there
is no output to hold. One step of ordinary code is a function from that string to the next; a device
evaluates it with classical reversible gates, as a circuit defined on every string of `s` bits, strings no run
reaches included; a run that has returned or faulted stays as it is. The function is in general not
invertible (a store overwrites a byte). So each step gets its own fresh **record** of `s` qubits, 0 at the
start: the step XORs the next state into its record, then exchanges record and working state, so that the
working state holds the next state and the record the whole previous one. That is a permutation of all basis
states, records that are not 0 included, and on a fresh record it does exactly the ordinary step. (The undo
journal of "Running a program" keeps only what a step overwrote; that is enough on one definite run, but it is
not this construction, and nothing here shows that it would do on qubits.) For each `apply`:

1. a working register holds the run's state: memory as it stands, a fresh frame for `@f`, the run marked as
   running; every record is 0 (the arguments stay in their registers, and the steps read them there);
2. the `B` steps run forward, each with its own record, then one final step with its own record, which marks a
   run still running as out of budget and leaves any other state as it is;
3. the result, or the fault code, is copied into `out` or `fault` by XOR;
4. the inverse of every step of 2 runs, in reverse order.

For each definite value of the arguments, the steps of 2 take the working register and the records from their
start, one basis state, to one basis state; step 3 changes only the targets, which the steps of 2 and 4
neither write nor use as controls; so step 4 takes working register and records back exactly to their start.
That start is the same basis state whatever the arguments hold, so at the end working register and records are
not entangled with anything and are released without effect. By linearity, on any superposition of arguments
and targets, with any other registers beside them, the result is the unitary of "Meaning" above. The
hypotheses, and what in the tree they rest on:

- *`@f` is ordinary code*: a quantum instruction, `effect` or `exit` reached inside faults before it acts
  (`crates/exec/src/forward.rs`). So each step is a function of the run's definite state alone, and with its
  record a classical reversible circuit, taking basis states to basis states; the permutation comes from the
  record, not from the faults.
- *The start is one basis state, the same for every argument value*: the part of memory accessible to the
  applied function is definite. The validator refuses a register where an integer is expected
  (`docs/SYNTAX.md`, "Registers are not integers"), a `load` or `store` at a byte that holds qubits faults
  `qubit_memory_access`, and a measurement's outcome is a definite value. The fresh frame, the position and
  the records do not depend on the arguments.
- *The targets are untouched by the steps of 2 and 4*, neither written nor read: `@f` can name no register,
  works on a private copy of memory in which touching a byte that holds qubits faults, and the copy of step 3
  is the only operation on `out` and `fault`. The tree enforces that they are distinct registers: the
  validator refuses `out` or `fault` among the arguments, or `out` equal to `fault`, and the executor faults
  `qubit_alias` when two handles name one register.
- *The outcome after `B` steps is a total function of the arguments and of the definite state before the
  `apply`* (its memory, and the caller's frames, which `retaddr` can read): every operation is total and every
  failure a named fault ("The device program"), and a run not finished after `B` steps gives `budget`.

**Cost.** On a device one `apply` is the `B + 1` steps of 2, the copy of 3 and the inverses of 4. Each step is
one circuit computing the next state from the whole state, the choice of instruction included, so its size
depends on that circuit and on `s`, not on what one run happens to touch (a step that can load at an address
depending on the arguments must reach every byte). With `B + 1` records of `s` qubits beside the working
state, all returned to 0, that is at most a constant times `B` times (the size of the step circuit plus `s`)
gates. These are upper bounds for this construction; nothing here says a cheaper circuit cannot exist. The
executor's step counts (`Stats`, `RegionCost`) count instructions, not gates; they are not a cost on a device.

**Gate emitter.** A **program point** identifies a function copy (the applied entry or the function
at a particular sequence of call sites), a block, and the index of its instruction or terminator.
In the **definite prefix**, all definite argument assignments still running share one next point.
The slot values there can still depend on qubits. A **proved constant** bit has the same value
for every assignment reaching its point on every visit. The compiler can use a proved constant
condition to select a jump even if other assignments have a different value in that slot.
The compiler represents each bit as a constant, a qubit's value or its complement. It computes
new bits by reversible gates into scratch qubits.

The emitter's full instruction subset is `add`, `sub`, `mul`, `and`, `or`, `xor`,
`udiv`, `sdiv`, `urem`, `srem`, `shl`, `lshr`, `ashr`, `icmp`, `select`, `zext`, `sext`, `trunc`,
`faddr`, unconditional and conditional `br`, `switch`, `ret`,
and direct `call` and `call_indirect` as described under "Copied calls" below. Calls take integer
parameters and may return an integer or no value. The [refusal table](#not-yet) gives the excluded
operations and the limits on calls. A jump reads all incoming values before assigning block
parameters. Entry stack overflow takes precedence over the budget test. Every successful instruction
or terminator costs one step.

Automatic mode first tries the in-place residue update described below. In the general
construction a qubit-dependent conditional jump can use **control gates**. A **guard** is a bit
that is one exactly on the definite argument assignments that take a route. The compiler combines
the incoming guard with the condition or its complement to form the two new guards. Each controls
the gates for its route, and the route's slot values are meaningful only where its guard is one.
If the routes rejoin, the compiler selects each assignment's slots from the enabled route and emits
the continuation once. Its analysis looks for the nearest common continuation of the routes to a
return, using only the selected target of each literal conditional or switch. If both routes could
emit shared code before reaching a common stop, the compiler uses the step machine, which represents
each assignment's current point with qubits. This covers short-circuit tests whose early returns bypass
the shared code. A loop whose test stays definite can use control gates for a choice in its body.

The compiler accumulates guarded returns and faults from different routes or steps. For each
definite assignment, exactly one outcome guard is one. The circuit XORs that result or fault code
into its destination, then reverses the computation and releases the scratch in zero.

At a join the compiler keeps the smallest and largest possible step counts. If it can
still charge the next instruction at the largest count, every enabled input may attempt it.
If even the smallest count has spent the budget, all enabled inputs get `budget`. If
the counts straddle the next attempt, the compiler abandons the control gates and uses
the step machine for the qubit-dependent part of the whole function. This preserves the
first return or fault for each input. Gates from the abandoned construction are removed
before compilation continues.

A **proved pass count** comes from the program. For a test of whether a block parameter
is zero, the compiler checks that every route from the nonzero outcome back to the test
passes a logical right shift by one of that pass's parameter. A word of width `w` must
then be zero after at most `w` passes. Each emitted pass uses one control formed by
combining the previous pass's running inputs with the new test. The test may also sit at
the end of a pass, compare the old unsigned word with two and pass its shift by one back
to the body. The compiler checks that another edge cannot replace the word before the
body reaches that test again. On an exit the compiler records the proved-zero word before
assigning the exit arguments. A later outer pass
that receives this zero skips the drained inner loop's body. Exits are collected at the
shared continuation, which is subject to the same budget check as any join. Proved
counts can nest. A loop with a qubit-dependent exit and no such proof uses the step
machine or the single-loop construction below. A route still cycling without a program
proof at the budget boundary also uses the step machine, so an enclosing proved loop
cannot copy a schedule whose length comes from the budget.

**Unsigned value bounds.** A word's maximum is obtained by treating every bit not
proved zero as one. Zero extension keeps its added bits definite. For a product
the emitter caps the working width at the bit length of the product of the
operand bounds, or the declared width if that bound overflows. Bits above the working
width are represented as literal zero and allocate no qubits. The operand analysis keeps
these high zeros for later proofs without generating the product gates.

**Products of qubit-held words.** Each bit pair contributes one AND to the column
of its two indices' sum. A half adder reduces two column bits and a full adder reduces
three, keeping the sum bit in that column and passing the carry to the next column.
Only the declared low bits matter. The gadget reserves its result, computes the
product, copies it to the result and reverses the computation. Its work wires return
to zero and can be reused by the next gadget. The full `apply` also reverses this gadget
when clearing the result.

**Variable shifts.** For each power of two `d` below the word width, the amount's
corresponding bit chooses between shifting by `d` and leaving the word alone. Arithmetic
right shifts fill with the sign bit. A comparison using all amount bits clears the result
when the amount is at least the word's width, including for arithmetic right shift.
A proved constant amount is wired directly, with no shift gates.

**Division and remainder.** Read the dividend from high bit to low bit. Each new bit
doubles the current remainder and adds the bit. A trial subtraction, with one extra
high bit, selects the difference when the candidate is at least the divisor. The
previous remainder is below a nonzero divisor, so one subtraction suffices for the new
remainder to stay below it. Each decision to subtract also gives one quotient bit, from
most significant to least significant. A quotient needs just the comparison on its last
trial because no later trial reads the residue. For signed operations, the circuit first
forms the unsigned magnitudes using conditional two's complement negation. It negates
the quotient if the signs differ, or negates the remainder if the dividend was negative.
The result uses the declared width, so the signed minimum divided by minus one wraps
back to the signed minimum.

For unsigned remainder, a known modulus makes
bits above its maximum residue definite zero. A power-of-two modulus takes only the
low bits. When the dividend's upper bound equals a known modulus, the comparison
with that value selects zero on equality and leaves the other values unchanged.
The general gadget copies its result and reverses the work, including on inputs
with divisor zero.

A separate zero test makes each reached division or remainder fault with
`divide_by_zero` for exactly those inputs. A failed input writes only the fault code and
stops before any later instruction. A successful attempt costs one step. If the zero test depends on qubits in the
definite prefix, the emitter records the fault under the zero-test guard and starts the step
machine at the next point for the successful inputs, with one step spent.
On a controlled route it records the fault under the route guard AND the zero test, then
continues on that route with its guard narrowed to the successful inputs. In the step machine
itself only the successful inputs receive a successor and the result write.

**Copied calls.** Each sequence of call sites has its own copy of the callee's slots and
program points. Before building gates, the compiler counts one unit for each slot, instruction
and terminator in the entry and each copy, including those in bypassed blocks. The same site
can run repeatedly using its copy. A successful call charges one step, reserves eight bytes per
slot below the caller, zeros the new frame and assigns the
arguments. A return charges one step, writes the result to the caller and continues
after that site. Both operations use the same budget as the applied entry.
An invalid definite address or signature gives `bad_indirect` before checking space
for a callee. A failed frame reservation gives `stack_overflow`. The budget is checked
before either instruction, and the entry frame check still comes before the first
budget check.

The planner creates copies only for sites the entry analysis can reach. An indirect
site with one proved definite address uses the same copy construction. Reachable
recursive calls and indirect sites without one proved address are refused before
issuing gates. A copied frame keeps the caller's proved literal bits and equalities
between argument bits across all visits to the call site. A call result is unknown to
the caller's operand proof. When the first qubit-dependent jump is in a frame entered
by the definite prefix, the control construction can follow calls and returns under
the route guard. A return from a copied frame stops at the caller's continuation
until sibling routes have been lowered. If a copied program first branches on qubits
in the entry frame, or routes reach incompatible continuations, the emitter can use
the step machine. A copied fixed-charge loop can use the single-loop proof below.

The **step machine** carries one **dispatch bit** per possible program point, set for the
definite assignments whose next instruction or terminator is there. In one **tick** it evaluates
the operation at each such point once from the slot values at the start of the tick. The
dispatch bits guard the writes and successor points. Only after all reads does the machine
install the new slot values and dispatch bits. Incoming edges to a point are mutually exclusive
per assignment, so XOR joins their dispatch bits. A returned or faulted assignment stops
dispatching. At the budget boundary each still-running assignment gets the code `budget`.

A `switch` with one possible edge follows that edge directly. The call planner and
dispatcher skip case values that disagree with known selector bits or with bits that
share a qubit. If the compatible cases cover every assignment to the selector's
distinct qubits, they also skip the default. A switch with multiple possible edges uses
the step machine. Each remaining case compares the full selector with its case value
and combines the match with the dispatch bit at that point. Case values are distinct,
so XORing the matched dispatch bits out of the incoming dispatch bit gives the guard
for a default that remains possible. Each target reads its arguments from the same
pre-tick frame, including when several cases share a target. The whole switch costs
one step.

Dispatch uses the steps left after the definite prefix, with the first qubit-dependent jump
or switch counting as the first tick when it triggers dispatch. All loops share that budget.
Nesting adds possible points to a tick. When no possible point remains, dispatch stops early.

When control gates encounter a loop without a proved pass count, the compiler can try
**saved decisions**, bits recording whether each definite input is still running at each loop test.
The first qubit-dependent jump may be the loop test or a choice between entering the loop and
returning early. At the loop test, one route goes back to the test and the other returns from the
applied function. Both routes are straight sequences of integer operations, unconditional jumps,
and returns from already-entered copied calls. The integer operations are `add`, `sub`, `and`, `or`, `xor`,
`icmp`, `select`, `zext`, `sext`, `trunc`, `mul` with a literal operand, and shifts. The proof counts the steps on each route,
including conditional and unconditional jumps and each return. At each test, a return is considered only
if its whole exit route fits in the steps left. A saved decision prevents an earlier return
from contributing again. A still-running input with no remaining affordable return must
exhaust the budget, because the skipped instructions cannot fault. Compilation reports the
construction and its reason.

For saved decisions, the compiler can enumerate all definite values of at most eight distinct
argument qubits and trace the instructions under the whole function's budget. This gives
an upper bound on the unsigned value written to each result slot. For low-bit arithmetic and
selections the compiler omits gates for bits proved zero, while keeping the declared word width
for later instructions. When every input stops, the schedule ends at the last reached return or
fault attempt. The analysis has a work limit. Above eight argument qubits, or if the trace would
exceed that limit, the compiler uses the original widths and the full remaining budget.

A pass count proved from the program may be used at any nesting depth. For example, a
logical right shift of a `w`-bit loop variable by one on each pass makes it zero after at most
`w` passes. The whole applied function still uses `budget B` to determine whether it runs
out of steps. The emitter may derive a pass count from that budget for one loop only.
It may never repeat a budget-derived inner schedule for each budget-derived outer slot.

The 8-bit Collatz test uses a 14-bit trajectory word and an 8-bit update count. It returns
after `5 + 9k` steps for `k` updates. The maximum trajectory value is 13,120 and the longest
terminating input takes 127 updates. Zero never returns. The bit-level runner checked all 256
inputs at 253 distinct budgets, including immediately before, at and after each terminating
input's return. It checked both zero and patterned destinations, unchanged inputs, and zero
scratch. The hand-written program's rows use `B = 1,148`. Counts include the forward gates, the
output and fault XORs, and all inverse gates. Peak includes the 21 input, output and fault qubits.
Depth counts layers of gates on disjoint qubits.

| Construction | One-qubit gates | Two-qubit gates | Toffolis | Peak qubits | Depth |
|---|---:|---:|---:|---:|---:|
| Step machine (1,145 ticks after 3 definite steps) | 8,410 | 53,269 | 65,688 | 26,888 | 44,745 |
| Saved decisions (128 decisions after 3 definite steps) | 7,136 | 28,513 | 19,258 | 14,330 | 3,729 |
| C Collatz, saved decisions (127 loop decisions at `B = 1,150`) | 14,152 | 36,875 | 20,252 | 15,327 | 7,001 |

Run `cargo test -p bennett-emit --test apply collatz_eight_bit_budget_sweep_and_counts -- --ignored --exact --nocapture`
to repeat the sweep and counts.

The C Collatz program uses an ordinary `unsigned` trajectory and returns zero for inputs
zero and one. The front emits a call to a helper that tests the initial value before its first
pass. Including the call and both returns, it takes five steps for inputs zero and one and
`7 + 9k` steps for `k` updates. The loop-carried value still fits in 14 bits. The front computes
the tripled value plus one before selecting the odd or even result, so that intermediate
needs at most 16 bits. The emitted circuit uses saved decisions and keeps one budget across
the wrapper and helper.

Run `front/test.sh --test emitted_collatz c_collatz_budget_sweep_and_counts -- --ignored --exact --nocapture`
to check the C row and compare the gates with the executor on all 256 inputs at 250 distinct
budgets, including immediately before, at, and after every return cutoff. A budget one full pass
beyond the last return produces the same gate list.

The next counts use the same convention and include the argument, eight result and five fault
qubits in the peak. The ordinary loop adds the three-bit input on odd inputs and subtracts it
on even inputs, three times. The unequal routes reach a shared return after two or three steps.
The shift loops return the bit length of the input, taking `4 + 5k` steps for `k` passes.
One nested example shifts a two-bit word and reloads the three-bit inner input in each outer
pass. The draining example instead carries the final value of its six-bit inner word across
passes of a six-bit outer loop. It emits at most six body passes for each loop. Each measurement
uses a budget that allows every input to return.

| Example and budget | Construction | One-qubit gates | Two-qubit gates | Toffolis | Peak qubits | Depth |
|---|---|---:|---:|---:|---:|---:|
| Ordinary loop, 31 | Control gates | 948 | 104 | 612 | 170 | 1,265 |
| Ordinary loop, 31 | Step machine | 460 | 326 | 354 | 181 | 639 |
| Unequal routes, 3 | Control gates | 3 | 0 | 0 | 14 | 1 |
| Unequal routes, 3 | Step machine | 6 | 6 | 0 | 14 | 12 |
| 2-bit shift, 14 | Control gates | 44 | 10 | 22 | 26 | 52 |
| 2-bit shift, 14 | Step machine | 78 | 19 | 53 | 35 | 81 |
| 3-bit shift, 19 | Control gates | 108 | 18 | 56 | 42 | 104 |
| 3-bit shift, 19 | Step machine | 134 | 49 | 132 | 62 | 165 |
| 4-bit shift, 24 | Control gates | 172 | 28 | 103 | 60 | 147 |
| 4-bit shift, 24 | Step machine | 190 | 95 | 242 | 99 | 273 |
| 5-bit shift, 29 | Control gates | 260 | 40 | 165 | 83 | 227 |
| 5-bit shift, 29 | Step machine | 254 | 157 | 383 | 146 | 405 |
| 6-bit shift, 34 | Control gates | 352 | 54 | 239 | 109 | 289 |
| 6-bit shift, 34 | Step machine | 326 | 235 | 555 | 203 | 561 |
| 8-bit shift, 44 | Control gates | 556 | 88 | 424 | 170 | 434 |
| 8-bit shift, 44 | Step machine | 494 | 439 | 992 | 347 | 945 |
| Nested shifts, 53 | Control gates | 288 | 106 | 357 | 139 | 405 |
| Nested shifts, 53 | Step machine | 1,468 | 2,810 | 5,169 | 1,675 | 1,527 |
| Draining shifts, 85 | Control gates | 868 | 248 | 847 | 310 | 1,039 |
| Draining shifts, 85 | Step machine | 4,744 | 9,342 | 18,285 | 5,614 | 4,939 |

Run `cargo test -p bennett-emit --test apply controlled_apply -- --nocapture` for the counts and
boundary checks. The simulator compares amplitudes with direct evaluation at the return
boundaries for shift widths 2 through 6 and for the nested example. The bit-level runner
checks every 8-bit input at budgets 0 through 45, with zero and patterned destinations, and
all 4,096 inputs to the 12-bit draining example at budget 85, with patterned destinations.
For 8, 9 and 10 short-circuit pairs with early returns, automatic mode emits the same
gate lists as forced dispatch, with 3,343, 4,544 and 5,999 gates respectively.
The literal-switch loop test selects the default edge back to an unchanged word.
Nonzero inputs exhaust the budget in both modes because that edge never shifts the word.

**Arithmetic counts.** The table counts native Toffolis for each gadget, including
the reversal that clears its own work wires after copying the result. An `or`
of two words with distinct variable bits uses one Toffoli per bit pair by
complementing the inputs and result of an AND. For a product
of two words with distinct variable bits, `P` counts bit pairs in the retained columns,
`F` counts full adders that produce a carry and `H` counts half adders that produce a
carry. The product uses `2(P + 2F + H)` Toffolis, or `6w² − 6w` for two words of width
`w ≥ 2` with a double-width result. The general remainder uses `6pm` for a dividend
with `p` possible significant bits and a divisor with `m`. A general unsigned quotient
uses `6pm − 2m` Toffolis. An eight-bit unsigned quotient at `B = 2` including its zero
test, output and fault XORs, and full reversal uses 2,331 two-qubit gates and 772 Toffolis
and peaks at 520 qubits. For independent `w`-bit signed operands, both trial widths
are `w`. The two input negations and one result negation add a total of
`6 max(2w − 3, 0)` Toffolis, including local reversal. The zero test for a divisor
of `m` distinct variable bits uses `2(m − 1)` for `m ≥ 1`. A known nonzero modulus
needs no zero-test gates, and a power-of-two modulus needs no reduction gates.

| Operand width `w` | Product, `6w² − 6w` | Remainder, `p = 2w, m = w` | Variable divisor zero test, `2(w − 1)` |
|---:|---:|---:|---:|
| 4 | 72 | 192 | 6 |
| 6 | 180 | 432 | 10 |
| 8 | 336 | 768 | 14 |
| 64 | 24,192 | 49,152 | 126 |

For a variable shift of an independent `w`-bit word by a `w`-bit amount, a logical stage
at distance `d` uses `2w − d` Toffolis. An arithmetic right stage uses `2(w − 1)`.
If `t` is the number of trailing zero bits of `w`, the range comparison uses `w − t − 1`
Toffolis, and clearing an out-of-range result uses `w`. The gadget doubles the sum of
these costs to clear its work wires after copying the result.

A switch at the start of dispatch with `k` explicit cases and a selector made of `w`
distinct variable bits uses `k(w − 1)` Toffolis for the case matches. The default guard
uses XOR gates. With separate targets that return constants, the complete circuit
uses `2k(w − 1)` Toffolis, including reversal.

The C Grover test uses the ordinary predicate that sums the decimal digits of an 8-bit
input and checks for 12. At `B = 40` and two Grover iterations, the emitted gates and direct
evaluation give the same amplitudes after each `apply` and before measurement. They also
give the same measured result with the same seed. Each emitted circuit releases its
scratch in zero.

The complete `mulmod` applies a double-width product followed by remainder with
modulus `2^w − 3` held in an ordinary value. Both modes emit the same gate list at
`B = 7`. For `3 ≤ w ≤ 64`, reversing both gadgets after copying the result gives
`2((6w² − 6w) + 6(2w)w) = 36w² − 12w` Toffolis. At width two the modulus is
one, so the remainder takes no gates and the complete circuit uses 24 Toffolis
for the product and its reversal. Peak qubits include both inputs, the output and
the fault register.

| Operand width `w` | One-qubit gates | Two-qubit gates | Toffolis | Peak qubits | Depth |
|---:|---:|---:|---:|---:|---:|
| 2 | 0 | 24 | 24 | 23 | 34 |
| 3 | 440 | 765 | 288 | 165 | 440 |
| 4 | 780 | 1,436 | 528 | 282 | 754 |
| 6 | 1,748 | 3,402 | 1,224 | 612 | 1,622 |
| 8 | 3,100 | 6,200 | 2,208 | 1,070 | 2,814 |
| 64 | 196,860 | 422,336 | 146,688 | 65,862 | 166,110 |

Run `cargo test -p bennett-emit --test apply controlled_apply::mulmod -- --nocapture`
to repeat the measurements. At widths 4, 6 and 8 the simulator compares exact
amplitudes with direct evaluation at budgets 6, 7 and 8. The bit-level runner checks
all operand pairs at widths 2, 3, 4, 6 and 8, and 128 sampled pairs and five corner pairs at
width 64. It checks unchanged inputs, XOR into the two destinations, and zero scratch.

**In-place modular multiplication.** The compiler can reuse a residue word in a
single shift loop. It recognises an unsigned product followed by a remainder,
including a direct helper that returns that pair of operations. The loop carries
the remainder back as its next residue. A conditional update can use the low
exponent bit and pass the old residue through the other route. The exponent shifts
right by one each pass, and the base is squared and reduced by the same modulus.
The test can sit before the body or at its end.

The proof checks the data flow before allocating the residue. The product has one
use. The old residue can feed the update, pass through the skipped route or leave
the loop before another update. The initial residue must be a known value below the
known modulus, which fits in 64 bits. The remainder preserves that bound. The
compiler checks the widths of the products, squarings and output conversions.
When checking a route, the compiler records the comparison value for that edge
before it transfers any block arguments. Each multiplier must have a computed
and verified modular inverse. An upper bound on the full route length, including
calls, returns, branches and exits, must fit the applied function's single budget.
The shift gives the pass count independently of that budget. Any failed proof
leaves the existing compilation and fault behaviour in use.

For an `n`-bit residue the gates accumulate the modular product in an `n + 1`-bit
temporary word by adding one constant per set residue bit under the update control.
Each modular addition subtracts the
modulus, restores it when the subtraction is negative, and clears that decision.
A controlled exchange puts the new value in the residue. Subtracting the product
of the new residue and the inverse multiplier clears the old value from the
temporary word. The controls leave the residue unchanged on an unused update.
The temporary and adder work qubits return to zero on each pass. At the end the
oracle XORs the residue into the target and reverses the computation to release
its work qubits.

Run `cargo test -p bennett-emit modular` for the gadget and the proof and fallback checks.

**Period finding from C.** The C square-and-multiply function receives the whole
eight-bit exponent register in one `apply` with `B = 96`. The compiler schedules
eight controlled passes using the same residue word through the copied call.
The test requires this in-place construction and checks the amplitudes immediately
after the oracle against direct evaluation, then runs Fourier gates in device IR
before the measurement written in C. The exponent probabilities agree with the exact
Fourier sum to within `5.6e-16` for 15 and `2.5e-16` for 21.

The hand-built oracle uses ripple-carry modular additions of constants to multiply
in place under each exponent bit. The adder omits the unused top carry. The inverse
multiplier clears the multiplication's temporary word. The oracle copies its residue
by XOR and reverses the computation. The bit-level test checks both circuits on all
256 exponents with zero and patterned destinations. It checks the preserved exponent,
the XOR of the expected residue, the fault bits, and the cleared scratch.

Gate counts and depth cover the exponentiation. Both gate lists use the same
cancellation of adjacent identical Toffolis and count a Toffoli as one native gate.
The isolated peak includes eight exponent, eight output and five fault qubits,
including the output's high zero bits. Scratch is the peak beyond those 21 qubits.
The harness peak is measured by running each oracle inside the same C harness with
the Fourier gates. It includes two further five-qubit fault registers that remain
live across the exponentiation.

| Modulus and base | Oracle construction | Toffolis | Isolated peak qubits | Scratch | Harness peak qubits | Oracle depth |
|---|---|---:|---:|---:|---:|---:|
| 15, base 7 | C, in-place updates | 1,552 | 37 | 16 | 47 | 3,695 |
| 15, base 7 | Hand-built, controlled modular multiplications | 1,552 | 37 | 16 | 47 | 4,015 |
| 21, base 2 | C, in-place updates | 9,240 | 40 | 19 | 50 | 22,459 |
| 21, base 2 | Hand-built, controlled modular multiplications | 9,240 | 40 | 19 | 50 | 24,059 |

Both C oracles match the hand-built Toffoli counts and full harness peaks. Before
the in-place update, the C oracles used 3,228 and 18,999 Toffolis and peaked at
543 and 763 qubits in the harness. They retained intermediate products, remainders
and guards across passes. The in-place construction reuses the residue and the
accumulator and adder work qubits. The benchmark enforces the factor-of-two limit
on both the Toffoli count and the full harness peak.

Run `front/test.sh --test emitted_shor c_shor_period_distribution_and_counts -- --exact --nocapture`
to repeat the comparisons and counts.

The counting and OpenQASM devices use the compiler for each `apply` they reach. Pass
`--apply-circuit auto` to run the same gates on the simulator, or `--apply-circuit step-machine`
to select instruction dispatch whenever control depends on qubits. These modes implement the
machine's fault semantics, writing fault codes into the program's register. The reference
simulator run is `bennett run --machine-meaning` with its default direct evaluation.

Before consulting the budget, the emitter checks every instruction and terminator in every
planned function copy, including those in bypassed blocks. It plans indirect calls only in
reachable blocks and requires a single proved function address at each such site. A proved shift
amount uses fixed wiring and a proved switch selector follows one target. A qubit-held amount
uses the shift circuit. A switch with several possible edges uses step machine dispatch.

Each function copy has one word of facts per slot, shared across blocks and visits. Within
a block, an operand can use the word just computed there. An absent word means its facts
are pending. The first word supplies the initial facts. When words meet, matching constants
and matching argument bits survive. Conflicts and other computed qubit bits become unknown bits
identified by slot and bit index. The analysis starts at the entry and follows the selected
edge when a conditional jump or switch folds to a constant. For a qubit-held condition it
follows both edges. For a qubit-held switch selector it follows the cases and default its
bit facts permit. It waits for operands whose facts are pending. Reachability can grow as
more facts arrive.
Refusals for given arguments apply at every budget, including zero.

The controller resolves the targets and arguments first, keeping dangling and aliased
handles as enclosing faults. It checks entry frame space, then builds and validates the
whole circuit before sending an allocation or gate for that `apply`. A compiler refusal stops
the enclosing run and leaves the result and fault registers as supplied. An unsupported-operation
error identifies the function, operation, block and instruction index, and the construction needed.
Other compiler errors report an invalid circuit interface or exhausted wire address space.

**Faults for some argument values.** With a fault register that starts at 0, measuring it afterwards finds a
fault with probability equal to the total weight of the faulting configurations, and the state collapses onto
the outcome found; nothing in the program learns more than that one measurement shows. The executor sees every
configuration.
It counts each `apply` execution that faulted for some configuration of nonzero amplitude, with their weight,
in the run statistics. By default such an `apply` stops the run with the run error `region fault`, leaving the
state as it was before the `apply`. Either `--allow-region-faults` or `--machine-meaning` lets the run go
on, for programs that read their fault registers on purpose (`examples/fault.bn`). Programs from `front` are
always run with the stop in the tests. The stop is the simulator's ("The simulator's own stops and reports",
below); the program's meaning is the fault code in the register.

## From C to a device program (`front`)

`front` is the only module that links LLVM. It reads the bitcode stock `clang` emits, runs LLVM's own pipeline
(without vectorisers), and writes the text form; the reader and validator in `ir` check what it wrote. The marker
surface is `docs/MARKERS.md`. Input LLVM's reader cannot parse, stops on or crashes on (damaged metadata), or its
verifier rejects, is named "the input is not valid LLVM IR" with status 2, and no device program is written; an empty file is LLVM's empty module and gives
an empty device program (`front/src/main.cpp`, `readInput`).

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
  not read x. Anything else that overwrites x keeps x's old value in a register that is released at once
  ("Overwrites that keep the old value" below), unless the old value can be computed again (next).
  After measuring the unfused Shor program, add a device form that specifies the update and its
  inverse. Its expansion computes the new value into a fresh register, exchanges it for the old
  value, and clears the old value by applying the inverse. The emitter can check the pair and fuse
  those steps. In the long term the optimiser should discover these opportunities itself.
- **An overwrite whose old value is a function of registers unchanged since** clears the variable by computing
  that value again. When x was last given a value `v` that depends on a qubit, by a whole store, and every
  qubit variable `v` reads (x not among them) is unchanged between the read and the overwrite, and every call
  in `v` sees the same memory there (`Control.cpp`, `recomputable`), the overwrite `x = g` is `apply @v -> x`
  (x is 0) then `apply @g -> x`; where `g` reads x, as `if (r) x = 7` stores `r ? x : 7`, that read is `v`'s
  computation. A load that read a known ordinary value (with `--taint-all` every load is treated as a qubit
  read) is given that value. Nothing is kept and no warning is written; the dataflow's third kind of fact,
  `Computed`, records `v` (`Taint.h`).
- **A one-bit update `x = e(x)` with `e(1) == e(0) ^ 1`** is `x ^= e(0)`. The identity is proved by LLVM's
  simplifier on two clones of `e`, or, where `e` reads no ordinary value and the other qubit variables it
  reads have at most `maxEnumeratedSupportBits` (8) bits that may be 1, by folding both clones on every
  assignment of those bits (`Control.cpp`, `provesFlipByCases`): InstCombine rewrites the negated `select` of
  `q && !r` by De Morgan into another `select`, so `(s ^ 1) ^ s` did not fold and `y ^= q && !r` was refused
  until 2026-11-17 (the fallback oracle's `coherent_xor`, and `coherent_nested` at O0).
- **Conditional jumps, joins and loops that depend on a qubit** are outlined into ordinary functions and
  applied. A store into memory in them makes the bytes it writes hold qubits ("Memory written under a qubit"
  below); a call in them that may write memory is refused by name. The control program applies an outlined
  function where its value is used, which may be after memory it read has changed (a span of held bytes may
  start in between). It reads a fixed address before the region and passes the value in only if LLVM's
  `isSafeToLoadUnconditionally` allows the read on every path and the front can establish that the bytes
  are ordinary there (`Outline.cpp`, `HeldBytes.h`). The front checks fixed offsets within globals
  against earlier exchanges, stores and quantum calls that can reach the same object, following incoming
  control flow and callers. When it cannot establish that the bytes are ordinary, the load stays in the
  applied function, subject to the memory checks in "Applied calls", "Where the value is computed".
- **Code without gates under such a condition that writes qubit variables is one applied function** for all
  the variables it writes, its loops and nested loops included ("Code under a qubit" below; `RegionResults.h`).
- **Quantum operations under such a condition are predicated** otherwise (`Predicate.h`). For each basis configuration
  the outlined code runs one path; its gates and stores are moved after it, in a topological order of the
  code, each acting only where its path reaches it. A gate becomes a gate controlled by "this configuration
  reaches it"; a store `x = v` becomes `x ^= (reached ? v ^ x : 0)`, or, when that still reads x, the
  straight-line `x = reached ? v : x` that the in-place update then inverts. Each condition and change is
  an ordinary function copied from the outlined code, control flow and all, so nothing is computed for a
  configuration whose path would not compute it. A condition that LLVM's simplifier proves to be a conjunction of
  single qubits becomes the gate's controls (a CNOT, a Toffoli); any other is computed into a fresh
  qubit, controls the gate, and is computed again to clear it. A gate whose condition reads the gate's
  own variable, a value read before an operation of the outlined code changed it and needed after, and a
  measurement are refused by name. A loop with gates is kept as a loop ("Code under a qubit", gate loops).
- **No new device instruction was needed** for any of this: gates take control lists, and `apply` XORs
  into any register.
- **Every `apply` has a fault register of its own**, released when the function returns (inside a loop: right
  after the `apply`), so the codes of two faults never cancel. A fault the run was allowed to go past stays in
  its register until the release, which drops it (the partial trace, "Releasing a register" below); the
  simulator reports that release.
- **The budget** (`--region-budget`) is the `B` of every `apply` the front writes. Code whose control flow
  depends on a qubit, a loop whose trip count depends on one included, runs under one `apply` and one `B` for
  everything it does (or, for several results, a pair of `apply`s that each run all of it, one pair for each 64
  bits of results), as does a function called whole. An update of a qubit variable inside a loop of the control program, whose trip count is
  an ordinary value, is one `apply` per pass: separate computations, one after another, not one computation
  cut up (the maintainer's decision of 2026-11-17).
- **Each qubit variable is Known** (it holds an ordinary value: fresh, assigned, just measured) **or Quantum.**
- **What it cannot do yet it refuses by name**, with the source location: a qubit-dependent value used by
  ordinary code or by an access with an effect of its own (an atomic update, a volatile or atomic read, even
  with its result unused), reading a qubit variable's old value after it changed, qubit variables passed to functions or
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
  5%) and 5% to 16% more device code (`docs/RECORDS.md`, "Known weak spots").
- **Aggregate results of at most 128 bits of integer and floating-point fields** (`{ i64, i64 }`, how
  clang returns a small struct in two registers; SoftFloat's `extFloat80_t`) come back as one integer, the
  fields side by side, so code under a condition on a qubit can call such a function. **Other aggregate and vector
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
  offset moves. `fneg`, `fabs`, `copysign` and `is.fpclass` are bit operations (`is.fpclass` as LLVM's
  x86-64 back end expands it, mask by mask, so the class of a noncanonical long double is native's:
  `FpClass.h`); `fmuladd` is unfused, as x86-64 without FMA computes it natively; `fma` is fused.
- **Where C leaves a choice, the native program's** is taken: `fmin`/`fmax` are clang's x86-64 expansion
  (`isnan(x) ? y : y < x ? y : x`, so of two equal zeros the first), `fminl`/`fmaxl` are glibc's, which the
  native program calls; an out-of-range conversion to an integer goes through the integer LLVM's x86-64 back
  end converts to (SSE 32 or 64 bits, x87 16 or 64), so it gives what native code gives.
- **The default runtime** is Berkeley SoftFloat 3e with the x86 SSE specialisation (`rt/softfloat`, measured
  in `rt/softfloat/README.md`); `long double` is computed at x87's full 64-bit precision. `--float-runtime`
  swaps the runtime at link time. Its global state: the rounding mode is never written and folds to
  round-to-nearest-even; the exception flags are never read (C reads them only through `<fenv.h>`, the C
  library's), its `softfloat_raiseFlags` does nothing, and the optimiser deletes them, so the routines write
  no memory and run under qubit control. Runtime calls on constants are evaluated after the optimiser,
  so a rotation's angle may be a computed constant.
- **Complex multiply and divide** (`Complex.h`): clang's calls of `__mulsc3`, `__divsc3`, `__muldc3`,
  `__divdc3`, `__mulxc3`, `__divxc3` go to libgcc's own sources (`rt/libgcc`, GCC 13.3.0 vendored unmodified,
  decided 2026-11-16), which are ordinary C on floating-point values: the helpers a program calls and does not
  define are linked from `complex-runtime.bc` before floating point is lowered, internal, and lowered with the
  program. `--complex-runtime` swaps them. All six are self-contained, including `__divxc3`; no C
  library is needed. Native references use the same libgcc release. Raw NaN-bit agreement remains a
  separate measurement from the non-NaN exactness guarantee (`rt/libgcc/README.md`).
- **Refused by name**: `frem` and the C library's functions (`sin`, `pow`, `strtod`, and `sqrt` under the
  default `-fmath-errno`, since errno is the C library's); operations on `__float128` (moving it is fine);
  conversions to and from integers wider than 128 bits; `powi` on `_Float16` and `__bf16` (natively the C
  library's `powf`); `#pragma STDC FENV_ACCESS`; an operation whose result depends on the subnormal mode
  (arithmetic, comparison, conversion from floating point, every intrinsic but `fabs`, `copysign` and
  `is.fpclass`) in a function that promises a mode other than IEEE through `denormal-fp-math` or
  `denormal-fp-math-f32` (`-fdenormal-fp-math=preserve-sign`, `positive-zero`, `dynamic`), since SoftFloat
  keeps subnormals where the promised environment flushes them (`Denormal.h`); moves and those bit operations
  stay accepted. Route: the floating-point environment through SoftFloat's state.
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

## Code under a qubit

Control code cannot branch on a value that depends on a qubit, so the front takes the smallest single-entry,
single-exit piece of code around such a branch or join (`Outline.h`). What happens to it depends on what it
contains. Without gates and without stores into qubit variables it is one ordinary function, applied once, with
one budget, whatever loops and calls it contains. With gates it is predicated (above). Stores into memory in it
make their bytes hold qubits, kept in a qubit variable while they do ("Memory written under a qubit").
**Without gates but with stores into qubit variables** it is also one applied function, for all the variables it writes
(`RegionResults.h`), with one budget for everything it does, loops and nested loops included; the front creates
no loop, counter or bound that is not in the C, and none of the code's loops stays in the control program.

For each qubit variable `y` the code writes, the applied function returns `final XOR start`: the variable's value
where the code ends XOR its value where it begins, computed for each definite value of what the code reads, with
`y` in a private local of the applied function that starts at

- **its known value**, when the variable holds an ordinary value before the code (it is fresh, was assigned an
  ordinary value, or was just measured): the code may then read and overwrite `y` as it likes; or
- **0**, when the code only accumulates into `y` by XOR: every value it computes from `y` is `y XOR d` for a `d`
  that does not read `y` (loads of `y`, XOR with what does not read `y`, extensions, truncations and masks that
  keep the bits `y` may hold, `!=` and `==` between values of at most one bit, joins of such values on
  conditions that do not read `y`), and nothing else uses them. Then the code's control flow does not depend
  on `y`, and its final value is `y XOR` its final value from 0.

The variable is then XORed with that result: `final XOR start XOR start = final`. One result is one `apply` into
the variable, the variable being the target. Several share a fresh register `N`: one `apply` into `N`, a CNOT
from each bit of `N` into the bit of its variable, the same `apply` again, `qfree N`. Neither `apply` has a
written variable among its arguments (their starting values are ordinary values or 0), and the variables the
code only reads are not changed between the two, so the second `apply` computes the same value and returns `N`
to 0. Each `apply` runs the whole code: two runs of `B` steps however many results fit in one register.
Afterwards a variable whose value before the code was known holds that value XOR its field of the result, a
value the front can compute again from the registers the code read while they are unchanged, as after one
`apply` into the variable: a later overwrite of it clears the old value by computing it again and keeps
nothing (the front records the value for its analysis, `bennett.holds` in `Taint.h`, which emits no code).

A register currently holds at most 64 qubits. When the results take more bits, they are packed into words of at most 64
bits, each a register of its own, and the one applied function takes the word's number as an ordinary argument
and returns that word: every word is computed and the one asked for chosen at the end, so each run takes the
same steps whichever word it returns. Each word is then one `apply` into its variable (a word of one variable)
or a pair around a fresh register as above, with the same arguments and the same `B`: two runs of the whole
code for each register. There is never a second copy of the code. The plan is to add wider registers
and held objects before Shor above 64 bits.

What LLVM moves out of the code is still the code's. When it promotes a variable that a loop of the code
writes to a register (scalar promotion), the stores in the loop become one store after it, in the block where
the code ends, of a value the code computes. That store counts as a write of the code, and the value is a
result of the same applied function, when the block is entered from the code only and begins with the store
(after phis and code that touches no memory), the store writes all of a qubit variable the code does not touch,
and the variable's value before the code is known: its result is `final XOR start` with the stored value as
`final`, and the store goes. Otherwise the store stays, and the value is computed again by the `apply` that
writes it (below, a debt).

A variable whose value before the code is not known and which the code does more with than XOR into it is
an overwrite of its old value. Without a loop in the code, predication handles such a store (an in-place
bijection, or the forms of "Overwrites that keep the old value"). With a loop, the variable is **exchanged**:
it is a store by exchange ("Overwrites that keep the old value") whose stored function `g` is the whole code,
with the variable's old value among its arguments. The applied function starts the variable's private local at
that argument and returns its final value, in a word of its own; with `x` the variable and `n` a fresh
register of its width,

```
n = qalloc w
apply @code(a, x, word) -> n        for each definite value: n = final value of x
x n[b] ctrl x[b]                    three CNOTs per bit exchange n and x:
x x[b] ctrl n[b]                    x = final, n = x_old
x n[b] ctrl x[b]
n ^= x_old                          where x_old is known or can be computed again (below)
qfree n                             0 if cleared; otherwise the old value, dropped (partial trace)
```

Several exchanged variables of one piece of code share the one applied function and take one fresh register
each: every final value is applied first, while all the old values are still in their variables (each run reads
all of them), then every exchange, then every clearing. The results XORed into their variables are applied
before the exchanges too. `n` is cleared with the old value where it was known, or by computing it again where
it is a value computed from registers unchanged since it was stored, none of them a variable of the group (the
clearing of a store by recomputation, `ControlWriter::recomputable`: a value stored before the code, returned by
an applied call, or left by earlier code under a qubit applied whole, an exchanged variable's final value,
computed from its old value computed again, among them). Then `n` returns to 0 for every definite
value of what the code reads, nothing is kept, and the code's loops, nested loops and calls run inside the
applied function under its one budget: one run of the whole code for each exchanged variable, and one
computation of each old value. An old value that is neither known nor computable again (`x` was changed by
a gate, or computed from a register that has changed since) is kept: `n` is released as it is, by `qfree`, the
partial trace, and the front warns, naming the variable and the qubits kept, one register of its width for
each time the store runs (the retained form of "Overwrites that keep the old value", with what it loses: no
interference between configurations whose old values of `x` differ, full coherence where they are equal; if
the applied function faults for some argument values, interference between those values and the rest can be
lost when the fault register is released). In one group some registers may be cleared and others kept.

A value the code leaves for later code that was read from a variable after the code wrote it is refused too,
as predication refuses it. Where LLVM keeps a loop's variables in registers through the loop and stores them
after it, the code writes nothing and each store after it may read a variable stored just before; such a read
reads, there, the value the variable held where it was read, computed again from registers unchanged since
(`ControlWriter::xorInto`). clang `-O2`'s own vectoriser can turn an accumulation into vector lanes, which the front
does not follow; the tests disable it in that loop (`front/tests/c/region_whole.c`).

A value the code leaves for later code is one of its results even when no instruction of the code computes
it: a value selected where the code ends by the edge it leaves on (an LLVM phi, a *join*, of constants or of
values from before the code, as for `if (q) a = 2;` when LLVM keeps `a` in a register), whose value is decided
by the path the code took. Such a join, like any value of the code used after it, keeps the code's control flow
in place for the computation that uses it (below), and a store LLVM moved to the join is a store of that value.

Debt: a value the code computes that later code uses in any other way (a plain local assigned in a loop under
a qubit and used in arithmetic after it, or such a join) is computed again, by a copy of the code inside the
`apply` of the computation that uses it: the result is right, but that `apply` runs the code and its loops once
more. Route: make the value a field of the code's results, held in a fresh qubit variable until its last use,
then cleared by the same results computed again while their arguments are unchanged, or kept as an overwrite
keeps its old value.

Acceptance (`front/tests/tests/region_whole.rs`): two nested loops whose trip counts depend on a three-qubit
register `x`, XORing into two variables of values the front does not know and counting into a third, agree
with native C for all eight values of `x` at O0, O2 and with `--taint-all`, every other register back at 0; so
do the parity of `x` accumulated into a `_Bool` and an overwrite in a loop of a variable of known value. The
control program has no loop left, and one pair of `apply`s into one register carries all three results (the
counter's stores are among those LLVM moves out of the loops). The smallest budget for all values is 150 steps;
at 149 the longest runs fault, the others do not. Results of 64 + 32 + 16 + 8 bits from one loop take one
applied function, one `apply` into the 64-bit variable and one pair for the other three, and agree with a hand
computation for every value of a three-qubit register in all three modes.
Exchanged variables (`front/tests/tests/qubit_differential.rs`, `loop_overwrite_*`): a loop whose trip count
depends on a three-qubit register overwrites a variable whose old value was stored before it, returned by an
applied call, or written (two variables, beside an accumulator) by earlier code under a qubit, and the form
LLVM keeps in registers through the loop; each agrees with native C for every value of the register in all
three modes, with nothing released that is not 0, and its state before measurement has native C's values with
the preparation's amplitudes and phases (a Z on one qubit), every other register at 0. After a gate on the
variable (`loop_overwrite_*keeps*`): native C's values, the warning and its count, exactly one release that is
not 0, of the variable's width, and the reduced state of the kept registers equal to the prediction, with
coherence exactly between the register's values whose old values are equal (all different; equal in pairs; one
variable kept beside one cleared in the same loop).
`front/tests/tests/qubit_differential.rs` compares seeded short C programs (conditional stores, updates, short
loops and helpers under a two- or three-qubit register) with native C for every definite value of the
register; among its fixed cases, joins of constants and of values from before the code, a store LLVM merged at
a join, a store LLVM merged at a join into memory whose span starts after the code that reads it
(`if (!q) a = 5; else { a ^= q; b += q; }` on globals), and a later update that reads a join agree with C in
all three modes; the same shape with the read of `a` in a helper the code calls is refused by name.

**Gate loops.** A loop with gates under a condition that depends on a qubit stays a loop of the control
program, run as written: the circuit model needs a definite list of gates, and the loop's trip count is an
ordinary value written in the C, so the control program issues the gates of every pass, controlled by the
condition that the code reaches the loop (`Predicate.h`). Nothing in the loop depends on a qubit (what does is
refused, below), so which gates act in which pass is decided by the loop's own ordinary code, which the control
program runs. The condition is computed once before the loop, by one `apply` into a fresh qubit (or is one
qubit as it is, `if (q)` on a `_Bool`), controls every gate of the loop in every pass, and is computed again
after the loop by the same `apply`, which returns the qubit to 0. Its applied function runs the path to the
loop and none of the loop's passes, so its steps do not grow with the trip count or with where in the loop a
gate is. Nothing is unrolled or created: the loop is the C's loop, entered where its ordinary guard holds,
and ordinary values it needs from the code before it are computed again before it. The guard is the ordinary
part of every test on the way into the loop, from the branch that enters the code under the qubit down to the
loop (each conditional branch one of whose edges every path to the loop takes): LLVM may have merged the
loop's own guard into the branch on the qubit, and put code of its own between them (a nested loop's guard
hoisted into a block before the outer loop). The loop also runs for the configurations whose path would not
reach it, as ordinary code that writes no memory, its gates acting on none of them. For each definite value of
what the condition reads the result is the product of the gates the C applies there.

The loop's ordinary code, and what the front computes again before it, therefore runs *unasked*: for definite
inputs on which C does not run it. It is accepted only from an allow-list (`Unasked.h`), each instruction
checked where the device runs it, for every input that reaches it there, using only the tests the device
makes there:

- it cannot fault there: integer arithmetic other than division, comparisons, choices, casts, address
  arithmetic and funnel shifts, which never fault on the device (a shift by too much gives 0, signed overflow
  wraps); a division or remainder whose divisor's range, computed with the device's wrapping arithmetic from
  constants, excludes 0 (`(n & 255) + 1`; a shift counts only where its amount is below the width, since the
  device gives 0 beyond, so `-3 >> n` proves nothing), or which a conditional jump of the control program tests nonzero on
  every path to the division (in the loop, or an ordinary test outside the qubit-dependent condition); a test
  inside the code under the qubit proves nothing, since that code becomes applied code;
- it changes nothing later code can see: no store and no call, except the marker that numbers a bit, the
  funnel shifts and byte swaps the front keeps as intrinsics until after outlining, and intrinsics that do
  nothing on the device.

Everything else is refused by name ("in a loop with gates under the control of a value that depends on a qubit,
whose ordinary code the control program runs for every input, ..."): a read of memory (it can be out of bounds,
or read a byte that holds qubits, where C reads nothing), a division by a divisor not proved nonzero (also one
computed before the loop under the condition, as `m = n / d` for `for (i < m)`), a reservation of stack, a
trap, a store, a call. Routes, not built: for what can fault, write the fault code under the control of the
loop's condition qubit; for what the loop needs from a call or memory, compute it under its condition in one
applied function before the loop (the table of control bits below).

Nothing is proved about the number of passes (the maintainer's decision of 2026-11-19). The loop is issued in
full whatever the qubits hold: where the logic guarantees that the condition is 0 and the count is huge, the
device runs every pass, its gates acting on nothing, which a larger step limit cures; a loop that never ends once
entered (an ordinary stride that never meets its bound, a subtraction by an ordinary zero) does not end on the
device for any input, also where C skips it. Where LLVM's loop analysis (ScalarEvolution) finds no count for
the loop or for a loop nested in it, the front warns at that loop that the gates are issued whatever the qubits
hold and that the loop must end for every ordinary input; the warning also appears for loops that do end but
whose count the analysis cannot find, and it is silent for a counted loop of `2^64` passes. An ordinary test
the control program makes before the loop skips it on the device too: one written outside the qubit-dependent
condition (`if (d != 0) if (q) loop`), or one the front keeps as the loop's guard. A test written inside the
condition may be removed by the optimiser where the condition implies it (`if (q & n) if (n) loop`: LLVM drops
`if (n)`, and for `n = 0` the device runs the loop). The loop then runs its full count on the device for that
input, `2^32` passes for `do { ... } while (++j != n)` at `n = 0`; the warning appears only where no count is
found, so a counted loop after a folded guard runs silently. Both are ignored witnesses in
`front/tests/tests/coherence_regressions.rs` (`a_folded_guard_keeps_*`) until ordinary guards under a qubit are
protected.

Refused by name, each with its route:

- a loop whose number of passes depends on a qubit: a circuit is a definite list of gates, so the number of
  passes must be written as an ordinary value, the condition tested inside the loop. Such a loop is a
  different program from `while (x > 0) { gates; x--; }`: it does not consume its loop variable, which keeps
  its value afterwards;
- a loop whose passes decide on values that depend on a qubit carried from pass to pass (`t = x; for (i < n)
  { if (t & 1) h(y); t = step(t); }`). First support constant counts when the table of control bits,
  one bit per pass per gate site, fits in 64 bits. One `apply` computes the table before the loop and
  the same `apply` clears it afterwards. Then support any ordinary count by recomputing that pass's
  decision with one small `apply` per pass. Recomputing the prefixes takes about `n²/2` passes of work,
  with a budget for each applied computation.
- a gate in the loop on a variable the condition reads. The table form is correct when every gate leaves
  what the table function reads unchanged (those registers may be controls; targets elsewhere). A loop
  whose gates change the tested register without ever changing whether the condition holds could later be
  accepted with one control qubit computed once and cleared by computing it again; everything else keeps a
  record of the passes, the retained case of an overwrite (not built);
- a store into a qubit variable in such a loop (not built).

Acceptance (`front/tests/tests/gate_loops.rs`): the front audit's witness `if (q) for (i < n) x(y)`, and a
computed condition with two gate sites, one on odd passes only, give the state computed by hand for n = 0, 1,
2, 3, 6 at O0, O2 and with `--taint-all`, every other register 0; the loop has one controlled gate line. Two
nested gate loops give the state computed by hand for every pair of trip counts 0 to 2, a zero outer or inner
count included, and a gate at pass 199 of 200 under a budget of 16 steps gives it too, in all three modes.
`front/tests/tests/coherence_regressions.rs`: a read of memory that holds qubits, a division and a remainder by
a divisor that is zero only where C skips the loop (in the loop, and computed before it), and a call in the
loop, and a division whose divisor is tested on another path, are refused by name in all three modes, where the
front used to fault on the inputs C skips (also a division by `-3 >> n`, and one after a test of the divisor
whose two edges rejoin before the division); divisions by a constant, by a divisor the loop tests, by a divisor of
range 1 to 256 and by one an ordinary test outside the condition guards give native C's values; the loop shapes
whose counts can be huge where C skips them (a 64-bit counter started at 2 where C would start it at 0 and run to
the bound, a widened 32-bit bound, a count read from a local before the qubit is prepared, a count compared with
the qubit), run at counts of 1 to 3, give native C's values without the warning; a loop whose stride is an
ordinary input, alone, nested in a counted loop, and inside `if (q & n) if (n)`, gives native C's values for
stride 1 and the warning.

## Overwrites that keep the old value

An overwrite of a qubit variable whose old value the front can neither XOR away (it is not known), undo by an
inverse (the update is not a bijection it can invert) nor compute again from registers unchanged since it was
stored (`x = x * x`, `x = 5` on an `x` prepared by gates, or computed from a variable that has changed since,
`*y = 0`) is accepted (the maintainer's decision of 2026-11-17). With `n` a
fresh register as wide as the variable `x` and `g` the stored value as an ordinary function of the qubit
variables it reads (`x` among them, as an argument):

```
n = qalloc w
apply @g(a, x) -> n          for each definite value: n = g(a, x)
x n[b] ctrl x[b]             three CNOTs per bit exchange n and x:
x x[b] ctrl n[b]             x = g(a, x_old), n = x_old
x n[b] ctrl x[b]
qfree n                      the partial trace of the old value
```

The exchange maps definite states to definite states one to one (`|a, x, n> -> |a, n XOR g(a,x), x>`), so
the whole is unitary until `qfree`. What `qfree` drops is the old value: where it was the same for every
configuration present, nothing is lost and the result is exact; where it differed, amplitudes belonging to
different old values no longer interfere in any later gate (the record distinguishes them). That loss is
physics, not a limit of the compiler: no unitary sends two definite states to one. Dropping the record at
once, at the function's end, or never gives the same statistics for everything else. The front warns, naming
the variable and the number of qubits kept: one register of the variable's width for each overwrite that
runs (the warning counts per executed overwrite). If an applied function faults for some argument values,
interference between those values and the rest can be lost when the fault register is released.

This is the form for qubit variables in registers; stores into memory reach it through their shadows (next
section). Code under a qubit applied whole that reads and overwrites a variable in a loop uses the same
exchange with the whole code as `g` ("Code under a qubit", exchanged variables); there the register is cleared
when the old value is known or can be computed again, and released as here, with the same warning, otherwise.

Acceptance: `front/tests/tests/retained.rs` (`x = x * x`, `x += x`, `x = x / 3`, `x = 5`, a conditional
square on a three-qubit `x`: the weights of the values of `x` are those of C, at O0, O2 and with
`--taint-all`; the warning's count); the fallback oracle's `outside_class` (`*y = 0` on a `y` in superposition:
exact basis results, exact Born weights of every preparation, 512 samples, the warning), un-ignored.

## Memory written under a qubit

A store into memory whose value, or whether it runs, depends on a qubit makes the bytes it writes hold qubits
and keeps its value in superposition there (the maintainer's decision of 2026-11-17); the compiler inserts no
measurement. The front (`front/src/HeldBytes.h`) names the bytes by their object, a global variable, the
memory a pointer argument points to, or a local variable, and their offsets (one shadow per object, so
where two objects may share bytes the front must know it; below): constant offsets, or, inside
code under a qubit without gates (which is applied whole), the offsets LLVM's bound on a position allows (a
loop over a small array, `memset`, `memcpy` of a length that depends on a qubit). The union of the bytes the
writes of one object may touch, currently at most 8 (one register), gets a qubit variable of its width, its **shadow**
`m`, for a stretch of the function, its **span**. With `p` the address of the bytes, `w` their width in bits
and `v` their old value:

```
%v = load iw [p]             the old value: the bytes are ordinary here
x m[b] ctrl [p:b]            three CNOTs per bit b exchange m (0) and the bytes:
x [p:b] ctrl m[b]            m = v, a value the front knows; the bytes hold qubits at 0
x m[b] ctrl [p:b]            (the first gate at a byte makes it hold qubits)
...                          every access to the bytes is an access to m
x m[b] ctrl [p:b]            at the end, the same exchange: the bytes hold the value, m is 0 again
x [p:b] ctrl m[b]
x m[b] ctrl [p:b]
```

Between the two exchanges `m` is a qubit variable whose value before the first store is known, so the forms of
qubit variables apply: code under a qubit without gates that writes it, loops included, is one `apply` with
one budget ("Code under a qubit"); a store elsewhere is XORed in, inverted in place, or exchanged into a
fresh register that keeps the old value ("Overwrites that keep the old value", with its warning and count).
Written out for one store `x = g(a, x)` this is the exchange form of a store at memory: the new value is
applied into a fresh register, exchanged with the bytes by gates on address-named bits, and the register is
returned to 0 because the old value is known (`v`, or a function of registers unchanged since, computed again,
or computed back by an inverse); where none holds the old value is kept and dropped by `qfree`. The exchange maps definite
states to definite states one to one, so the span is unitary on the registers and the bytes except where the
retained form releases a register; the shadow starts at 0 and ends at 0 for each definite value of what it
depends on, outside those releases.

Where the span starts and ends. It starts before the first such write, outside the code under a qubit that
contains it. A store of a value that depends on a qubit that runs before the first code under a qubit on every
path, and is in no loop (so no qubit controls it), is held first: a later write under a qubit into the same
bytes is then an access to the same shadow, and the last write is the one the bytes keep, as in C. It ends before each return and before each `bennett_measure_bytes` that touches the bytes (the
programmer's measurement, `docs/MARKERS.md`): there the bytes get their value, and the bytes measured are
ordinary memory again, holding the result ("Qubits in memory"); the others keep holding qubits, so ordinary
code may then touch the measured bytes and not the others. Where a path on which the shadow still holds the
bytes joins a path on which the span has ended (a measurement on one path, a return after the join) or never
started (an ordinary condition skipped the write, and nothing after the join touches the bytes but a
measurement), the span also ends on the edges from the first: the same exchange, on those edges, or in a block
of its own after code under a qubit when the edges leave that code.
Any other disagreement between paths moves the start earlier, to a block that dominates, until every access
to the bytes sees one state on every path.

The exchange that starts a span must run only on ordinary paths on which a write of a value that depends on a
qubit into those bytes runs: on another path the language says the bytes stay ordinary. Two placements can
break this: code under a qubit is applied whole, so its exchange is before it, and an ordinary condition inside
it can skip a write (`if (q) { if (n) g = q; }`, which LLVM makes one condition `n && q`, or a loop over an
ordinary count that may be 0); and moving the start earlier may take it above an ordinary condition that skips
the write. So where the start ends up the front checks, byte by byte and as an allow-list, that every ordinary
path from it writes each byte it takes: a block is *sure* for a byte when, whatever the ordinary values, some
configuration of the qubits writes that byte from it before the function returns, by a store of exactly that
byte (a constant position covering it) whose value, or whether it runs, depends on a qubit; or when it jumps
unconditionally or on an ordinary value only to sure blocks, or on a value chosen by qubits alone (read as free,
as the text reads) to at least one. A jump back into a loop is credited nothing, and a write of another byte of
the object proves nothing about this one (a store counts when its value depends on a qubit, or when its block
depends directly on a jump on such a value). Where the check fails the span is accepted only if, after the
exchange, nothing but code under a qubit, such stores, reads whose value goes only into such a store back into
the same bytes (the old value of an update LLVM made `x = q ? v : x`) and measurements of the bytes touches them, the bytes are measured before every return, and the front can show they are ordinary before the
exchange (no earlier call or exchange may leave them holding qubits, `keepOrdinaryLoads`); then on a path that
skips the write the shadow holds the old value and the measurement gives it. Everything else is refused by
name: an ordinary access in between (it would read or write the shadow where C touches ordinary memory, and a
copy would make other bytes hold qubits), a return at which the bytes may still hold qubits, bytes not shown to
be ordinary before the exchange (route for all: one exchange on each path that writes, not built). Where the
check holds, an ordinary read between the exchange and the first store on each path that makes all the bytes
hold qubits (`if (!n) dest = g;` before a later `g = r + 2`) is refused by name, since it would see the shadow
where C sees memory; reads the allow-list above names are not, and an ordinary store there only sets the shadow,
which the later write or the end of the span settles. A jump on a value chosen by qubits alone one of whose arms
writes the bytes writes them on that ordinary path, as the text reads.

Which pointers may reach the bytes (`front/src/Aliasing.h`). What a pointer argument may point to is the union
of what the calls of its function in the module pass; a function the host may call (external linkage), or one
called through a pointer, may be given any address. A pointer the front cannot follow (loaded from memory,
returned by a call) may point into any object whose address escapes. An argument never points into a local
variable of its own call, and a `restrict` argument shares no bytes with another pointer unless its address
escapes (a pointer loaded from where a copy of it was stored is based on it, and may reach its bytes). Where every call of
a static function passes the same pointer for two arguments, the two are one object. A call reaches what its
pointer arguments point to and what its callee, through the functions it calls, accesses, a
`bennett_measure_bytes` in a callee included (it reads the bytes it measures). For a local variable of the
function the shadow takes `v` by XOR instead and the stack bytes are left as they are, so they never hold
qubits after the function returns; the exchange happens only before a measurement of all of them, after which
`m` holds `v` and is returned to 0 with the function's other known variables.

What is refused, by name: ordinary code that needs a definite value from the bytes while they hold qubits
(for printing, a jump or an exit code, the front asks the programmer to write the measurement), a call that
may read or write them meanwhile (given their address, or reaching their object
directly, as a function reading the global variable does), an access through another pointer that may reach
them, stores under a qubit through two objects that may share bytes (the route for these two: a static
function whose calls pass distinct objects or the same pointer, `restrict`, or a check of the addresses at run
time, not built), a second shadow of the same bytes while the first holds them (an earlier store of a value
that depends on a qubit that does not run before the code under a qubit on every path, or that is in a loop;
route: one shadow from the earliest write, not built), a callee that measures them (route: measure them in the
caller first; for the front, ending the span before a call whose only access to them is that measurement, not
built), after an exchange that runs also where C does not write them an ordinary access before their
measurement, a return at which they may still hold qubits, or bytes not shown ordinary before it (above), an
access that touches them only in part, a
position that is not constant outside code applied whole, more than 8 bytes of one object (planned
with wider registers), an object the
front cannot name, and a store into memory in code that also has gates (each listed in `HANDOFF.md`, "What is refused"). The
front assumes the bytes are ordinary where a span starts: if an earlier call left them holding qubits, the
load of `v` faults as `qubit_memory_access`. The front refuses by name if an externally callable
function's pointer arguments may alias bytes that hold qubits. The bytes the front
cannot rule out are the bytes held: where LLVM's bound is wider than what the C writes (`memcpy` of 2 or 4
bytes is bounded at 6), the extra bytes are bytes no configuration writes, and the span is refused by name
where they could still hold qubits at a return (above; route: prove which bytes a write at a position that is not
constant covers, and hold only those). If an applied function faults for
some argument values, interference between those values and the rest can be lost when the fault register is
released.

Acceptance (`front/tests/tests/memory_held.rs`): the fallback oracle's `store`, `memset`, `nested` and `loop`
fixtures (its `memcpy` fixture is refused by name, as above), with the cells measured by `bennett_measure_bytes`, give native C's q, r, y and whole
twelve-byte image (guard bytes included, bytes that still hold qubits read from their registers) for every
definite input, with no register released that is not 0, and native C's exact weights of every result for
the oracle's ten preparations, at O0, O2 and with `--taint-all`; a store under a qubit followed by its inverse
under the same qubit leaves `q` unentangled (a Hadamard then gives 0 every time); a straight store of a value
that depends on two qubits, a local array, an overwrite whose old value is computed again from the unchanged
`q` (nothing kept), and the same after a gate on `q`, which keeps its old value (8 qubits, the warning), give
C's values; an earlier straight store and later code under a qubit writing the same bytes (one shadow, the last
write kept, nothing kept); and the refusals, a second shadow, a copy of a `restrict` argument read back and a
callee that measures held bytes among them. `front/tests/tests/held_span_independent.rs`: whole and partial
measurements, a span ended by a measurement on one path and a return on another, the overwrites that clear
by computing the old value again (exact amplitudes, nothing kept) and the one that cannot, a region writing
two objects (every bit of each change), pointers that may share bytes (refused by name; merged, distinct and
`restrict` accepted with C's values), and a callee reading the global variable (refused by name).
`front/tests/tests/coherence_regressions.rs`: a store under an ordinary condition, read ordinarily by the caller on
the path that skips it, gives C's 0 (it faulted before); the same store under a qubit and an ordinary condition,
in a loop over an ordinary count, in a loop every pass of which may skip it, and a write of one array byte beside
a skipped write of another, are refused by name where the bytes leave the function holding qubits; measured
before the return they give C's values, but are refused where an earlier call left the bytes holding qubits or
an ordinary copy reads them in between; a later store on every path lets the exchange cross the condition (C's
values, the old value kept with the warning), and an ordinary copy before that later store is refused by name.
`front/tests/tests/held_regions_independent.rs`: two held objects of unequal width (exact amplitudes), a second
region over held objects, gate barriers before an overwrite, pointer pairs and call chains, seeded short loops
against native C, a straight store before a region writing the same byte (C's value), a callee measuring a
held global and a callee reading a saved copy of a `restrict` argument (both refused by name).

## Applied calls

The language rule, in plain words: **a call whose arguments depend on a qubit, to an ordinary function, is
applied whole; the writes it makes are undone when it ends, and only its result survives.** An *ordinary
function* is one of the program's functions that performs no quantum operation, directly or through anything
it can call (`controlFunctions`, `Outline.h`). The rule follows from what `apply` is: for each definite value
of the arguments the function runs on a private copy of memory, its result is XORed into the target, and the
private memory is discarded. This is the permanent meaning of an applied call. Calls that must share
state are written inside one applied function, where they see each other's writes on the same private
copy. If the caller reads that memory after the `apply`, it sees its value from before the `apply`.
Nothing more is needed for a function that writes memory than for one that only
reads it, and nothing about the function's size: musl's allocator, its stdio buffers, the file model, the
clock counter and the entropy image are ordinary device code over device memory (`rt/musl/README.md`), so
under `apply` they run on the private copy and are undone with it.

- **One budget for the whole call.** The `apply` has one step bound `B` (`--region-budget`) for everything the
  function does: its loops, nested loops whose trip counts depend on the arguments, its calls, allocation and
  file operations. There is no bound per loop and no unrolling. A value of the arguments that needs more
  than `B` steps gives the fault `budget` for that value only; the others get their result.
- **What is a fault inside.** Console output (`effect`) and `exit` cannot be undone; reached inside the
  applied call they are the named faults `effect_in_region` and `exit_in_region` for the values that reach
  them. musl writes its stdout buffer out at a newline: a `printf` of a line inside
  is the fault, while output without a newline stays in the buffer, which is memory and is undone with the
  rest (`front/tests/tests/applied_calls.rs`).
- **Where the value is computed.** The front applies the computation of a value where the value is stored
  into a qubit variable or controls a gate, not where the call is. So the memory the call reads must be the
  same there (`ControlAnalysis::memoryUnchangedBetween`; refused by name otherwise). A value used several
  times is computed again for each use, which gives one value for arithmetic and reads of unchanged memory; for
  a call whose value depends on the frames it does not, a known defect ("Evaluations the front repeats or moves",
  below).
  The applied function's remaining loads and its callees' accesses are both checked. Loads moved before
  the region are passed in by value ("Conditional jumps, joins and loops" above).
  The memory a call reads is what it, and the functions it calls, may reach outside their own frames
  (`Aliasing::reachesOutsideFrames`): a local variable of a callee exists only in a frame the call creates, so a
  function whose only accesses are to its own local variables (a `volatile` scratch value, or one a helper it
  calls fills through a pointer) reads no memory that can change, and its writes are seen by no other call.
- **What the front refuses.** A call whose result is not used, to a function that may write memory: applied,
  it would do nothing at all, while its writes are its only purpose (also when LLVM has removed the unused
  result and the call, which passes nothing by address, returns nothing). Two such calls in one computation when one may write memory outside
  the frames of its own calls: under the rule each starts from the memory as it was, but in one applied
  function the second would see the first's writes; the route is one register for each call's result, computed and
  cleared around the computation that uses it when a program needs that shape. The front also refuses
  a memory-writing call under a qubit-dependent condition ("Conditional jumps, joins and loops" above).
  Its writes would survive the condition in native C.
- **The rule is about calls**, so what a program computes depends on its calls reaching the front as calls.
  With the project's clang flags (`-O0 -Xclang -disable-O0-optnone`, `front/test.sh`) clang marks every
  function `noinline`, and the front's own pipeline removes `noinline` only together with `optnone`, so every
  call stays a call (an `always_inline` function is inlined, as the programmer asked). IR compiled otherwise
  (clang `-O2`, say) may arrive with a small callee already inlined: its writes are then stores in the
  calling function with qubit-dependent addresses or values, which the front refuses by name today. The front
  does not yet check where IR came from; mark such a callee `noinline`, or compile with the project's flags.
- **What the warning means.** Natively the writes would remain, so a program whose later code reads what the
  applied call wrote computes something else on the device. The rule is the language's, not a defect, but
  where the front sees the case it says so: a warning (the program is still lowered) when the called function,
  or a function it calls directly, writes a global variable or memory passed to it by pointer that code after
  the call in the same function reads, directly or through a function it calls directly (`Applied.h`). Writes
  through other pointers (into the heap, say) and reads through indirect calls are not followed.
  Extend the warning beyond direct callees when a program needs it.
- **`--taint-all`** does not move an ordinary call with ordinary arguments that may write memory into an
  `apply`: there its writes must survive. A call of a function that reads memory at most is treated as
  computable coherently and is moved into the applied functions that use its value (an evaluation, counted as
  one in "Evaluations the front repeats or moves").
  Such a call does not count as one of "two calls that depend on a qubit", and in the applied function it
  runs before anything that depends on a qubit, so it reads memory as it is at its own site, never what an
  applied call in the same computation wrote on the private copy.

The acceptance target of the rule is SQLite (`experiments/sqlite-on-device`, stage 4): C entries that set a
database up with ordinary code and then apply one query whose key depends on a three-qubit register; each of
eight queries gives the native result on each of the eight rows, and memory is unchanged across each `apply`
(`front/tests/tests/sqlite_applied.rs`, ignored by default since it needs the SQLite sources).

## Evaluations the front repeats or moves

The front applies a computation where its value is consumed, not where C computes it: once per consumer, and
once more where a register is cleared by computing its value again. Each such `apply` runs a function the front
generates, which makes the calls the C code made. A run of an applied function is fixed by the function, its
arguments, its budget, the frames below it with their positions, the stack pointer, and the bytes of memory it
reads and whether they hold qubits (`docs/APPLY-STEP.md`). Two runs equal in all of these give the same result or
the same fault, whatever the function looks at. So (`front/src/Evaluations.h`):

- **Copies.** Arithmetic and reads of memory unchanged between the places (checked as in "Applied calls") may be
  computed by as many generated functions as there are consumers. Anything else is one *evaluation*: a call, a
  reservation or release of stack, a saved stack pointer, a return address. Its value may depend on the frames
  around it (the address of a local, `__builtin_return_address`, `__builtin_frame_address`), and copies in
  generated functions of different shape run under different frames.
- **One evaluation, one generated function.** The front names every evaluation of a function with quantum
  operations before it outlines anything, and every copy it makes keeps the name (a copy of code under a qubit
  for each of its results included). A generated function whose
  text equals one written before is that function, so computing a value into a register and clearing it again by
  the same computation is one generated function applied twice. Not yet: an evaluation whose value C consumes in
  two ways (one call whose value C stores twice, or stores and uses again after the code under a qubit) is still
  computed by one generated function per consumer, and a value that depends on the frames (a frame address, a
  byte of a pointer to a local) can differ between the copies: a known wrong result with ignored witnesses.
  Route: compute it once into a register of its own, which every consumer reads and the same `apply` clears.
- **The same stack pointer.** Where one generated function with an evaluation is applied at two places, no
  reservation or release of the control function's own stack may lie between them (a variable-length array, a
  `stackrestore`); refused by name otherwise, in every mode (an ordinary call `--taint-all` moves in counts).
- **Frames that C does not have.** Code in a generated function runs under a frame the C program lacks. Nothing
  applied may reach, through its calls, `__builtin_return_address(N)` with `N >= 1` (a call through a pointer
  may reach every function whose address is taken): an ancestor's return address would be the generated
  frame's. Refused by name, also where the read stays below the generated frame (route: count call depth, or let
  generated frames not count, a change to the device program). A return address or saved stack pointer of the C
  function is not moved into code applied under a qubit (refused by name; route: compute it before and pass it
  in). Label addresses need nothing: computed goto is lowered before outlining to block numbers that are
  constants of the module, which moved code sees unchanged.

Acceptance (`front/tests/tests/coherence_regressions.rs`): a full return address computed into a word and cleared
by computing it again gives 0 in the word; a replay across a variable-length array and a helper comparing its
caller's return address one level up are refused by name in all three modes (each gave a wrong value before);
arithmetic with two consumers is copied and gives C's values with the preparation's amplitudes. A call feeding
two stores, and a frame address or a byte of a pointer to a local feeding two words, are ignored witnesses of the
defect above.

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
- **`longjmp %buf, %v`** checks the record against the live frames: a frame of the run (inside an applied
  function: of its run) at that depth, of that function and entry stack pointer, a `setjmp` at that position,
  and a stack pointer within that frame's allocations, at or above where they stand now. Anything else is the
  named fault `bad_longjmp`, never a jump somewhere else: a jump to a function that has returned (undefined in
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
  jumps restores the state exactly, checked step by step (`crates/exec/tests/setjmp.rs`).
- **In an applied function** a jump within it works for every argument value, the record living in the
  private memory; a jump to a frame outside it is `bad_longjmp` for every argument value. From C, a
  qubit-dependent call of a function that uses `setjmp` and `longjmp` within itself is applied whole like any
  other call of an ordinary function ("Applied calls"; `front/tests/tests/setjmp.rs`).
- **A `setjmp` in a function with qubit variables** is refused by name. The front's facts about each qubit
  variable (its value known, or held in its register only) and its checks that a variable or memory is
  unchanged between two points follow the function's control-flow edges; a `longjmp` resumes the function
  after its `setjmp` from any later point, or from a callee, along no such edge, so a read after the
  `setjmp` was taken for the value of the first return. A debt, with its route: give each `setjmp` the
  resumption edges from every later call and `longjmp` that may reach it, in the facts, in those checks, and
  in the blocks counted as being on a cycle (which take a fresh fault register per `apply`).

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
the rest of the environment and is all in the buffer too. Whole-buffer copies and partial copies at the
64/72-byte record/mask boundaries agree with glibc in the retained O0/O2 matrix, including where native
musl differs. Partial copies inside the record remain a defect: copying bytes 0..16 changes the device
continuation but preserves glibc's, because the two layouts place it at different offsets. The ignored
witness and three-way native comparison are documented in `rt/musl/README.md`, "Copied jump buffers". clang's `__builtin_setjmp`/`__builtin_longjmp`
(`llvm.eh.sjlj.setjmp`/`longjmp`) become the same instructions with the value 1; the frame address clang stores
in the buffer, `llvm.frameaddress(0)`, is the stack pointer at the function's entry (a caller's frame address is
refused by name). The address of a library `setjmp` or `longjmp`, a call through another prototype and an
`invoke` are refused by name. The fault is `bad_longjmp` (16), a code of its own since the maintainer widened
fault codes to 5 bits on 2026-11-16; before, it shared `bad_indirect` (4) with `call_indirect`, because the
4-bit code space was full.

## Return addresses (`__builtin_return_address`)

What C observes of a return address (`llvm.returnaddress`) is an opaque code address: every call from one
site gives the same one, calls from different sites different ones, and it is not null for a frame that was
called; the number itself, and anything at a level above 0, is the machine's business (natively level 1 and
above follow the frame-pointer chain: with frame pointers omitted, clang 18 at -O2 on x86-64 reads another
word or crashes, and a sibling call skips a frame). The call site lives only in the executor's frames, so no
C routine and no rewrite in the front can give it; instrumenting every call to store its site in memory would
add a memory write to each, which the front refuses under a qubit, and still could not give level 1. The device has
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
- **In an applied function** its frames sit on the run's own, so the applied function returns to its
  `apply` and a higher level reads the frames below. Those are the same for every argument value; the applied
  function's own frames may differ between argument values, and each run reads its own. A function the front
  outlines (`main.apply`) is a frame the native program does not have, so the address of a call made from it
  is not the native one; it stays opaque, so nothing C defines depends on it, as long as one evaluation is one
  generated function and nothing applied reads a return address above level 0 (both refused by name otherwise,
  "Evaluations the front repeats or moves").
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
  2^16 argument values per program.
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
  faults as `exit_type_mismatch` (14). Inside an applied function it faults first as `exit_in_region` (13).
  Success changes only the outcome, journals the same finish record as an outer return, and leaves active
  frames in place for reversal. C's `_Noreturn void __bennett_exit(int)` lowers to it; like
  `void __bennett_effect(char)`, it is an ordinary device call, not a quantum marker.
- Fault codes fit in 5 bits (`Fault::CODE_WIDTH`, widened from 4 on 2026-11-16, the maintainer's decision); a
  fault register is at least that wide, so every fault register of a program is a `q5` or wider. Fifteen of
  the 31 nonzero codes are used, the last `qubit_memory_access` (17); 11 was `dirty_free` and 15
  `bad_rewind`, both removed on 2026-11-17. Those codes stay unused and the remaining codes keep their
  numbers because programs may have measured the old codes.
- `qfree` is the partial trace and never faults ("Releasing a register", below).
- `measure` reads a whole register, or the `N` bits from bit 0 of the integer at an address.
- The simulator's own stops and reports (dropped amplitudes of rounding size, the configuration limit, the
  stop at an `apply` faulting for some argument values, amplitudes in snapshots, the report of nonzero
  releases) are listed below, "The simulator's own stops and reports".

### Qubits in memory

Gates take addresses, so a qubit can live at any bit of memory (`crates/exec/src/cells.rs`; the plain form kept
by the maintainer on 2026-11-17). Registers stay as they are, one way of naming qubits among others.

Qubits in memory are a **derived form**, defined by an expansion into `qalloc`, gates, `measure`, `qfree` and
ordinary code. The ordinary control code keeps a table from byte addresses to 8-qubit registers (the cells,
below), data of its own that no instruction of the program reads. Then:

- a gate at `[ADDR:BIT]` looks up the byte `ADDR + BIT / 8`; if it has no register yet, it allocates one
  (`qalloc`), applies `x` to each qubit whose bit is 1 in the byte (a definite value), stores 0 to the byte and
  enters the register in the table; then it applies the gate to qubit `BIT % 8` of that register;
- a `load`, a `store` or a jump record first looks its bytes up in the table and faults `qubit_memory_access`
  if one is there;
- `measure iN [ADDR]` copies the qubits of the cells among its `N` bits into a fresh register, one controlled
  `x` each, measures that register, releases it (it is then in a basis state, so the release is exact), and
  puts the result together with the memory's own bits.

The simulator does the same directly (`cells.rs`), reading its table only at the addresses an instruction
names.

- **Text** (`docs/SYNTAX.md`, "Qubits in memory"). A gate's qubit is `%r[i]` or `[ADDR:BIT]`: bit
  `BIT % 8` of the byte at `ADDR + BIT / 8`, that is bit `BIT` of the little-endian integer at `ADDR`, as
  `load` and `store` see it; `ADDR` is an `i64` value or constant, `BIT` below 128, `[ADDR]` is bit 0.
  Registers and memory mix in one gate. A qubit outside memory is `out_of_bounds`; the same qubit twice
  is `qubit_alias` (the validator reports the same address operand and bit twice); inside an applied
  function a gate is `quantum_in_region`.
- **What a qubit in memory is.** The memory of a basis configuration is the classical memory, except at the
  bytes that hold qubits, whose values are the configuration's. Such a byte is a **cell**: an 8-qubit
  register bound to the byte's address, part of the register table. The classical memory holds 0 under a
  cell. The first gate at a byte that is not a cell **promotes** it: a register is allocated, the byte's
  value XORed into it and the memory byte set to 0; the gate's journal record keeps the address and the old
  byte.
- **Which bytes hold qubits.** A byte holds qubits from the first gate that names any of its bits until that
  gate is undone, or until a `measure` at an address measures all eight of its bits: then it is ordinary
  memory again, holding the result (the maintainer's decision of 2026-11-17). Its qubits are in a definite
  state after that measurement, so the executor XORs the result out of them and releases them, which is
  exact. A byte measured in part keeps holding qubits. Today it stays that way even if its qubits
  become definite by other means or its stack frame returns. The planned device instruction
  will drop the qubits in the addressed bytes unexamined by partial trace, as `qfree` does for a
  register. A device tracks which bytes hold qubits in an ordinary table of addresses.
- **Ordinary code touching a byte that holds qubits faults `qubit_memory_access`** (code 17; the maintainer's
  decision of 2026-11-17, "yes, it should be a fault to read qubits"): a `load` or `store` any byte of which
  holds qubits, a `setjmp` record written over one, a `longjmp` record read from one. It faults in every
  configuration, whatever the qubits hold, before changing anything, so it is an ordinary fault of the run,
  as `out_of_bounds` is. The byte beside one that holds qubits is memory as before.
- **Inside `apply`** the applied function is ordinary code and the same rule holds: touching a byte that
  holds qubits faults, for each argument value, with the code XORed into the fault register (by default
  the simulator then stops at the `apply`, as for any fault for some argument values). To apply a function to
  qubits in memory, a program copies them into a fresh register with one controlled X per bit
  (`x %c[i] ctrl [ADDR:i]`), applies the function to the register, copies again to return the register to
  zero, and releases it: an expansion into gates, `apply` and `qfree`, used by the Grover test with x in
  memory (`crates/exec/tests/memory_qubits.rs`).
- **Undoing.** A gate at addresses is undone by its inverse, then by un-promoting the bytes it promoted:
  the old byte is XORed out of the register, the register un-allocated and the byte written back.
- **Measurement at an address** (the maintainer's decision of 2026-11-16): `%d = measure iN [ADDR]` reads
  the `N` bits from bit 0 of the integer at the `i64` address, little-endian, as `load iN` reads them, and
  gives an `iN`. The bits of bytes that hold qubits, and only those bits, are sampled jointly, in one draw
  (the same as measuring them one at a time, since those measurements commute, and as the expansion above),
  and the state collapses onto the values found: the other bits of a cell keep their superposition, and
  cells that are correlated stay correlated. The bits of other bytes are the memory's, with nothing to
  sample. Inside an applied function it faults `quantum_in_region`; reversal stops before it, also when no
  byte held qubits. An address whose `N` bits do not fit in memory faults `out_of_bounds`. Checked
  (`crates/exec/tests/measure_memory.rs`): outcomes, collapse and the bits left in superposition written out
  by hand. Grover with x in memory measured at its address gives the register form's answer for every seed
  (`memory_qubits.rs`).
- **A cell is a whole byte**, so a qubit in memory holds 8 qubits in the simulator, and the rule above is
  per byte (route: cells of single bits, if the count matters).
- **Checked.** Every step at qubits in memory is a bijection by brute force (`permutation_tests.rs`): gates
  that promote bytes at addresses that depend on an argument (in bounds, out of bounds, aliasing), `apply`
  beside cells, and a load, a store, a `setjmp` record and an applied function's load at a cell (each
  the fault), with an empty and a nonempty undo journal: 4,504 single steps (512 faulting).

## Releasing a register

`qfree %r` is the partial trace over `%r` (the maintainer's decision of 2026-11-17: "qfree can only mean
partial trace"): the register is dropped unexamined, whatever it holds, and its handle is dead from then on (a
later use faults `dangling_register`) and is never reused. A register that is not zero when it is released is
no fault of the program: `qfree` is the partial trace for every state of the register, and whether the register
was 0 is a property of the whole state (its nonzero part may have arbitrarily small weight), which the
instruction does not test. The
partial trace of a register entangled with others leaves them in a mixture; a program that wants the
interference back returns the register to zero first (uncomputes it), as the front does for every qubit
variable whose value it knows.

How the simulator stores the result (`crates/exec/src/quantum.rs`) is its own affair:

- If the register is zero in every stored configuration, its column is removed.
- Otherwise the column is kept, unnamed (its generation moves on, so no handle names it) and never reused.
  The stored state is then a **purification** of the state after the partial trace (a larger state whose
  partial trace over the kept columns is that state): what any later step of the program, a measurement
  included, can find out is the same either way.

Both are the partial trace exactly, as operations on the stored state. The amplitudes themselves are `f64`
values, with those of rounding size dropped after each gate ("The simulator's own stops and reports"), so what
is stored approximates that state as every state of the simulator does.

**The simulator's report.** `Stats::nonzero_releases` counts the executions of `qfree` whose register was not
zero in some stored configuration, and `Stats::nonzero_release_weight` sums the weight (squared amplitude) of
those configurations; `Stats::leftover_qubits` counts the qubits kept so. `bennett run` prints a line when the
count is not zero. No program can read any of it. A test may fail on it, as the front's tests do
(`front/tests/src/lib.rs`, `run`): it is a tool of the test, not a result of the program. Reversal takes a
kept register back, so undoing a release restores the stored state. Checked in `crates/exec/tests/release.rs`, each
expected state by hand: half of a Bell pair released (weight 1/2, the pair kept in its column), a release of
weight about 10^-12 reported as such, a stale handle, and a register returned to zero whose column goes.

## The simulator's own stops and reports

The executor is a simulator of the model, not a device. What it adds of its own is listed here; none of it is
part of the program's meaning, and none produces a value a program can read. Checked for each in the code:
no instruction reads `Stats`, a snapshot or a `RunError`; a stop ends the run; and inside an applied function
every quantum step faults `quantum_in_region` before any stop could arise, so no stop becomes a fault code
(`crates/exec/tests/strict.rs`, `no_stop_or_report_of_the_simulator_becomes_a_value_of_the_program`).

| What | Where | Why no program can read it |
|---|---|---|
| Amplitudes of magnitude at most `f64::EPSILON` are dropped after a gate (one unit of rounding, what cancellation leaves) | `quantum.rs`, `NEGLIGIBLE_AMPLITUDE` | A dropped configuration is never an outcome, and no instruction reads amplitudes. Each dropped amplitude is at most `f64::EPSILON` (weight about 5e-32), but the simulator does not sum what it drops: over many configurations and gates the losses add up with the other `f64` rounding, unbounded by that figure. The state is approximate in `f64` throughout (the tests compare amplitudes and weights with hand-worked values to tolerances from 1e-9 to 1e-20, most at 1e-12): evidence, not proof |
| The limit on stored basis configurations | `Options::max_configurations`, `RunError::ConfigurationLimit` | A stop: the run ends with no result, the state as before the gate |
| The stop at an `apply` faulting for some values of its arguments (strict mode, below) | `RunError::RegionFault`, `region.rs` | A stop. With `allow_region_faults` the fault code is the program's, in its fault register: that is the meaning, not a report |
| Amplitudes, the register table and cells in snapshots, and the distance `bennett run --reverse` prints | `snapshot.rs`, `crates/cli/src/run.rs` | Data for tests and reports; no device observation, read by no instruction |
| The report of nonzero releases (count, weight, qubits kept) and the other counts of `Stats` | `executor.rs`, "Releasing a register" above | Read by tests and printed by `bennett run`, never by an instruction |

The step limit of a whole run (`Options::budget`) is a parameter of the run, like its memory size: reaching it
ends the run with the fault `budget`, so a run given another limit is another run. Inside `apply`, `budget B` is the program's own bound, and its fault code is the program's.

**Strict mode** (the maintainer's decision of 2026-11-16): a check of the simulator that no device can make,
because it looks at every configuration, is a diagnostic, as a sanitizer's is, and strict mode, which makes it,
is the default. The one such check is the stop at an `apply` faulting for some values of its arguments.
`allow_region_faults` (`--allow-region-faults`) and `Options::machine_meaning` (`--machine-meaning`) each turn
it off. Both options stay. In `crates/exec/tests/strict.rs`, an `apply` out of budget stops the run
in strict mode. With `machine_meaning` the outcome matches `allow_region_faults` seed by seed.

## What is promised about coherence

The guarantee for ordinary code that depends on qubits was set on 2026-11-13 and widened on
2026-11-18. **Coherence** is the ability of amplitudes of different basis configurations to
interfere. Keeping a record of an old value can prevent interference between configurations
with different old values.

1. **Classical results are exact.** If the state was classical before, the C code runs, and you measure, you get
   what the C program says. By linearity the same holds for each definite value of a superposition measured at
   once. For the device program this is what `apply` means ("`apply`"); that the front lowers C to a device
   program with the C program's meaning is the toolchain's own claim, checked against native builds
   ("From C to a device program"), not something the argument for `apply` shows.
2. **Coherence is guaranteed for the shapes listed below.** The class may grow and may never shrink.
   Name each added shape and mark every shape **proved** or **tested only**. The front must warn
   when a program uses a tested-only shape. Supplying the missing proofs remains future work.
3. **Everywhere else, how much coherence survives is unspecified.** The toolchain may keep more or less of what
   was overwritten as it sees fit. Leaving it entangled is not a bug and not refused; the toolchain warns.
4. **Un-running code after something changed what it read or wrote** is likewise not refused. The result is
   unspecified and the toolchain warns.

| Shape | Status | Guarantee |
|---|---|---|
| Gates | Proved | A gate is unitary and retains no record. |
| `^=` when the expression does not read the target | Proved | XOR with a definite result is reversible, and `apply` clears its scratch. |
| In-place bijections listed in `docs/MARKERS.md` | Proved | The inverse clears the old value in the temporary register. |
| A computation run and then reversed with everything it read unchanged | Proved | The inverse operations undo the forward operations. |
| A store at a definite address whose old value is known or can be recomputed | Proved | The exchange is unitary and clearing the old value returns the spare register to zero ("Memory written under a qubit"). |
| Gate loops under a qubit with an ordinary trip count | Tested only | Each definite input gets the gates prescribed by the loop and the control register is cleared. |
| Updates inside loops under a qubit, with the old value known or recomputable | Tested only | The resulting values retain the preparation's amplitudes and phases. |
| Applied calls with qubit-dependent arguments | Tested only | On success the C lowering preserves the amplitudes and phases and keeps only the result of the call. The device-level `apply` construction is proved above. |
| Overwrites that retain the old value, including those in loops | Tested only | Interference survives exactly between configurations with equal old values ("Overwrites that keep the old value"). The exchange and partial trace give this prediction, and the C lowering is tested against it. |

These guarantees cover successful operations. Releasing a fault register can also remove
interference between inputs that fault and those that succeed.

The added shapes are covered by the state comparisons in `front/tests/tests/qubit_differential.rs`,
which check phases as well as weights. The recorded test coverage and its limits are in
`docs/RECORDS.md`, "Randomised differential runs". The front already warns when an overwrite
keeps an old value. The warning for use of a tested-only shape and the missing proofs for gate
loops, loop updates, applied calls and retained overwrites are tracked in `HANDOFF.md`, "Debts".

## Files, clock and entropy at the musl boundary

`rt/musl/vfs.c`, `devices.c` and `models.c` carry the earlier workspace's ordinary C models
(`docs/SALVAGE.md`, section 1; provenance and retained licence: `rt/musl/CARRIED.md`). They run on
the device; they never call host files, host time or a host random generator. There is no executor
special case: namespace entries, inodes, sparse 4096-byte pages, open descriptions, shared offsets,
descriptor flags, cwd, umask, the clock's read count and entropy's byte position are ordinary memory.
Stores and allocator changes are journalled by the existing machine. Un-running a read restores its
buffer and consumption position; un-running a write restores file bytes, length and offset.

The maintainer's 2026-11-16 decision fixes the inputs: unsigned 64-bit clock start (nanoseconds), and
an exact seeded entropy byte image. Reading the clock returns `(start + reads_so_far) mod 2^64` and
increments the count once. The clock is not execution time: no instruction or scheduler tick advances
it. The clock ids accepted by the model share this counter. `clock_getres` reports one nanosecond and
consumes no read; invalid ids consume none. `gettimeofday` divides nanoseconds into seconds and
microseconds, `time` into seconds. Entropy is a finite tape shared by `getrandom`, `getentropy`,
`/dev/random` and `/dev/urandom`. Exhaustion is a named `unreachable` fault before any destination or
position changes; readv preflights the whole tape request. This is an input image, not a PRNG or a
claim of cryptographic entropy. The measurement seed remains independent.

`rt/musl/link.py --clock-start N --entropy-image bytes.bin --file-image files.json` supplies a run
image. The JSON maps canonical absolute regular-file names to hex bytes, with parent directories
created automatically. Defaults are clock zero, empty entropy, and no user files. Images currently
enter at link time; changing them requires relinking. They are inputs even if LLVM specialises a
constant load. A future separate image loader can preserve these semantics with a symbol/relocation
map and mutable input bindings; it needs no change to clock or file semantics. Initial file installation
is lazy at the first device syscall, guarded by ordinary memory, so constructors see the supplied files
and full reversal restores the reset image including its initialisation guard.

The VFS supplies open/openat/creat, close/dup, scalar and vectored IO, positional IO, seeks, stat,
fcntl descriptor/status flags, truncation, readlink/readlinkat errors for the present non-symlink
namespace, directories/cwd, rename/unlink, chmod/umask and sync over
memory. `/dev/null` and `/dev/zero` are present. Standard fd 0 is empty, fd 1 is the existing journalled
console, and fd 2 retains its explicit nonempty-write fault even through duplication. Closing a standard
fd makes its number reusable. There is no disk backend without a use for one; block devices, locks,
links/symlinks, directory enumeration and unmapped system calls remain named faults, with the route
for each in `HANDOFF.md`, "Debts". Metadata uses a model device/inode identity, root ownership and zero
timestamps, rather than borrowing host-dependent values. This does not implement all Linux filesystem
semantics; allocation failure, permissions, metadata and namespace lifetime debts are recorded there.

For the smallest positive subnormal double, musl currently prints `0x1p-1074` with `printf("%a")`.
glibc prints `0x0.0000000000001p-1022`. Both spellings conform to C. Matching glibc through
musl's hexadecimal formatting path is a low-priority debt (`front/tests/tests/printf_hex_float.rs`).

`rt/musl/test-devices.py` is separate from the fast suites. It compares determined file results with
native from the same byte image, clock conversion/wrap and tape consumption with an independent
native C arithmetic oracle, and runs each device fixture forward and back against the executor's entry
snapshot. Runtime-specific tests are compiled without vectorization, as the front's C input requires.

## Floating point (decided 2026-11-15)

Decided by the maintainer on 2026-11-15: floating point is Berkeley SoftFloat 3e behind the compiler runtime-call
interface (the libgcc names), swappable at link time, for `float`, `double` and the x87 80-bit `long double`.
The reason: it covers `float` and the 80-bit format, it is a verified reference, and it needs no hand port. The
original Bennett.jl's branch-free routines are kept as a later, cheaper alternative for the optimiser, to be
checked bit for bit against SoftFloat. Mathematics functions (`sin`, `exp`, `pow`, ...) come from the C
library's own code; `sqrt` and `fma` from SoftFloat.

Non-NaN results agree bit for bit with the native x86-64 build. Two NaN results agree when each
carries the payload (the extra bits of a NaN) of an operand's NaN, quieted if needed, or of the
default NaN that an invalid operation produces. Their signs may differ. Including the default NaN
accounts for 1,476 of 164,096 `__divdc3` test cases that would otherwise disagree.
Measured against this machine's SSE
and x87 (`front/tests/tests/softfloat.rs`): no other difference in any operation or conversion, NaN bits
included, except for out-of-range conversions, which C leaves undefined. `long double` encodings no arithmetic
produces (pseudo-denormals, unnormals, pseudo-infinities, pseudo-NaNs, written through memory) are read as x87
reads every operand: a pseudo-denormal as the denormal of the same value, the others as invalid operands
giving x87's indefinite (`rt/softfloat/README.md`, "Noncanonical long double operands"). Where the native
program calls software rather than x87 instructions (glibc's `fmal`, libgcc's truncations to `_Float16` and
`__bf16`), that software reads such encodings at face value, and the device differs there. glibc's `roundl`
and `roundevenl` are software too; the runtime's follow their bit operations on these encodings.
`powi(x, 1)` returns its argument's bits, noncanonical ones included, as libgcc's loop does
(`rt/softfloat/README.md`, "Independent conformance pass").

## Text form

Line-oriented and writable by hand, in the style of LLVM assembly. No hashes, no embedded JSON, no version
negotiation. The exact syntax is whatever the single reader in `ir` accepts; `examples/` shows it.

## Tests

- `examples/*.bn`: hand-written programs with expected results, including the thesis program and an interference
  test (apply the function twice, undo the rotation, measure 0 every time).
- **Reverse restores everything.** After forward-then-reverse the test harness compares full snapshots (memory,
  slots, stack, tape, amplitudes) that it took itself. The executor's own say-so is not the check.
- **`apply` is the permutation it claims.** For small widths, exhaustively over every argument value, every `out`
  value and every `fault` value, compare against evaluating the function directly.
- Random programs, forward and back.
- `cargo test` finishes in seconds. Throughput and journal bytes per step are *reported*, never gated on wall time.

The independent test helper `crates/exec/tests/compute_copy_reference/` executes the reversible construction
for small applied functions. Its state vector includes argument and target registers, a fixed machine encoding
(value slots, call frames, program counter, step count and byte memory), and one full snapshot register per
scheduled step. Each step XORs its ordinary update into its record and swaps the machine and record registers;
the inverse swaps first and XORs the same update. After exactly `B` steps, including idle steps after a return
or fault, it XORs the result or fault code into the targets and runs all inverse steps. Every machine and
record bit must return exactly. This helper has its own interpreter for the small ordinary instruction subset
used by its tests; it shares the public IR reader, not executor stepping code, and is not a circuit compiler.
Exact complex integer amplitudes with a common denominator of 8 check all 64 prepared configurations of the
argument, result and fault registers, including relative phases and superposed targets. They are compared with
the executor within `1e-14` per real or imaginary component. Cases cover arithmetic, private memory, calls,
input-dependent loops and two nested loops, division and bounds faults, `effect`/`exit`, and a single budget
exhausted only for some argument values. This is finite test evidence, not a proof for all device programs.

The same helper checks stores by exchange on explicit amplitudes. It runs the applied function into the spare
register, exchanges it with the target by three controlled X gates per bit, and clears the spare by X gates
for a known old value or by a second applied function for a recomputable old value. The tests compare the
joint amplitudes with the intended store and a zero spare, including a target byte at a definite address.
For a retained old value and for a fault inside an applied function, they compare every reduced density
matrix entry after releasing the spare and fault registers. Coherence survives between matching pairs of
released values, even between faulting inputs, and is lost between differing pairs, even when the spare is
zero.

The IR mnemonic tests (`crates/ir/tests/mnemonic_contract.rs`) specify all 36 words independently of
the production macro, check each reader variant and writer instruction against that specification,
and reject missing operands. Shared reader/writer swaps cannot hide behind a round trip. The explicit
macro-expansion mutation trials and their failing tests are recorded in `docs/RECORDS.md`, "IR and CLI mutation pass (2026-11-16)".

Independent machine-decision probes (`crates/exec/tests/machine_decisions/` and
`crates/ir/tests/machine_decisions.rs`) cover every integer measurement width, partial-cell complex
amplitudes at unaligned addresses, byte/end-overflow bounds, every fault code with dirty targets and all 256
single-bit jump-record corruptions, against expected values worked by hand.

Independent fp-front probes compare powi's extreme integer exponents, long-double rounding at each
binary point, and classification at all five widths against native `-O0` and `-O2` builds. Three
ignored minimal witnesses record lroundl/llroundl's unsupported-encoding differences and optimized
native bfloat signbit's incorrect zero. Input admission is also checked through stdin and files,
including preservation of an existing output on failure. Scope and counts: `rt/softfloat/README.md`,
"Independent fp-front follow-up".

Tests constrain behavior, with witnesses that would fail for the intended defect:

- Derive small expected results independently, by hand, a dense calculation, an exact arithmetic oracle,
  or native execution. Reusing the implementation's classifier, allocator, or producer as the oracle
  only checks agreement with itself. Corrupt the purportedly verified input and require rejection.
- Exercise admitted syntax end to end. Parsing, a variant name in a test, or one successful execution
  does not prove the operation's result, refusal, or fault. Compare the complete expected outcome and
  exact error kind, including unused functions and unreachable blocks when the contract covers them.
- Vary every operand that should affect an answer. Include unequal, nonzero values and dirty output
  and fault registers; repeated XOR can hide a wrong fault through cancellation. Prove that a seeded
  defect is observable before trusting a test. Mutate changed Rust lines before a step lands, separately
  from the fast suite (`docs/CHECKS.md`); retain regressions for real survivors.
- At each reversible step compare complete independently captured state: memory, slots, frames, stack,
  tape and amplitudes. Test forward/reverse/forward, nested `apply`, faults and failed rollback;
  final return values alone miss corrupted state. Compare an applied function over every small input,
  dirty output and fault value with direct evaluation and an independent state oracle.
- Permute function and block order for semantic analyses; repeat calls and vary call paths. An analysis
  must not inherit an answer from whichever function or predecessor happened to be visited first.
- Exhaust small finite domains. At boundaries use below/at/above cases, zero and empty input, extreme
  widths and alignment, aliasing, repeated uses, nested control flow and resource exhaustion. An empty
  discovered test set or all-skipped run is not evidence of success. Large generated and corpus sweeps
  stay separate from the seconds-long suite, with a small executed regression retained for each finding.
- Check numerical boundaries against exact arithmetic or native bits, including signed zero, NaNs in
  every operand position, infinities, subnormals, rounding ties and directed modes. An ulp tolerance can
  hide a wrong rounding side. For numeric conversions/comparisons include values equal as f64 but
  unequal exactly, including neighboring integers beyond 2^53. State any allowed NaN or phase equivalence.
- Generate ABI matrices across scalar widths, vector shapes, mixed and homogeneous aggregates,
  alignment, by-value copies and variadic/direct/indirect/prototype-mismatch calls. Compare actual
  native bytes, statuses and output, with the pinned compiler; a hand-maintained list of familiar types
  missed entire classes. Verify that input/output or argument aliases cannot silently change meaning.
- For a checker, test both false acceptance and false rejection: broken certificates, stale artifacts,
  a different pushed revision, malformed/empty maps, and adversarial tokens where lexical checks are
  appropriate. A passing old log is never a check of new source. Keep documented numbers reproducible
  from result data; distinguish skipped, refused, faulting, crashing and wrong-result runs.
- Scale at least one parser/search witness beyond the usual small fixture when investigating recursion,
  stack or asymptotic cost. Report throughput, work and memory separately from correctness; do not gate
  correctness on noisy wall time. A measurement must exercise the path whose cost it claims.

The ten automatic checks, their entry points, seeded witnesses, pin scope, and hook installation are in
`docs/CHECKS.md`. Each uses exact expectations or source/compiler discovery, without a maintained
example list, allowed-hit count, mutation percentage, or performance threshold. Gaps stay explicit debts.

## Not yet

The optimiser, lowering the full device language to gates, certificates, target descriptions, a binary
format, and recomputation shared between conditions (each predicated operation computes and clears
its own).

Memory written under a qubit at a position that is not constant: the front holds the bytes LLVM's bound allows,
which can be wider than any write (`memcpy` of 2 or 4 bytes is bounded at 6), and refuses by name where such
bytes, never written, may still hold qubits at a return (the fallback oracle's `memcpy` fixture,
`front/tests/tests/memory_held.rs`, `a_held_range_wider_than_every_write_is_refused`). Route: prove which bytes a
write at a position that is not constant covers for some configuration whatever the ordinary values, and hold only
those.

| Refused inside emitted `apply` | Construction needed |
|---|---|
| Reachable recursive calls or adding a call copy that raises the total above 16,384 slots and program points | Shared call frames and return positions |
| Reachable `call_indirect` without one proved constant address | Prove the target across visits or dispatch among supported addresses |
| Calls with register signatures, `retaddr` | Register-valued call frames and code addresses |
| `store` | Memory in the step machine |
| `load`, `alloca`, `stacksave`, `stackrestore` | Private memory and stack state |
| `setjmp`, `longjmp` | Private memory and non-local control transfer |
| `fault`, `effect`, `exit` | Emit the named first fault on reaching the instruction |
| `qalloc`, `qfree`, gates (on registers or memory), `measure`, nested `apply` | Emit `quantum_in_region` on reaching the instruction |

## Open questions

- **Budget unit.** Steps should be counted in something the source fixes (executed instructions of the normalised
  LLVM bitcode), so that a better lowering never changes which inputs fault. How the plugin tells the executor which
  instructions count is undecided.
- **The adjoint of an applied computation that is not its own inverse.** Gates under qubit control and
  bijective in-place updates are done (above) without one.
- **Reversing ordinary code later.** A narrow static form of `mark` and `rewind`, with each mark
  paired with its rewind in the text, has a proof and may be added with its check
  (`HANDOFF.md`, "Decisions in force"). A program that needs to reverse gates writes the
  forward and inverse sequences explicitly.
- **Channels.** A dirty discard is the partial trace, measurement is described by effects, noise by channels; the
  finished Bennett covers them (maintainer, 2026-11-13). This step has unitaries, measurement and the partial
  trace (`qfree`, since 2026-11-17), the simulator keeping a purification. Nothing in
  the device program should make channels impossible to add.
- **Exact amplitudes** for the permutation tier.
- **Integers wider than 128 bits.** The device program has integers up to 128 bits (decided 2026-11-13: Rust's
  `u128` gives their semantics; splitting them in the front would need a type legaliser, which LLVM has only in
  SelectionDAG). Wider ones are refused by name.
