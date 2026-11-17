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
6. **Semantics and cost are separate.** What an `apply` *means* is defined for each definite value of its
   arguments. How much a circuit for it *costs* belongs to the optimiser, which must show it implements the same
   permutation.

## The model

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
- A **step** is one executed instruction or terminator of the device program.
- A **named fault** is a way for ordinary code to end: a code from 1 to 31 (`crates/ir/src/fault.rs`), part of
  the result, never a crash.

The model has three parts.

1. **Ordinary code**: integer arithmetic, memory, jumps, calls, console output, stop. Every operation is a
   total function of the machine state (its slots, frames, stack pointer, position, memory and output), with
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

Rotations take any angle written as a finite `f64` (`docs/SYNTAX.md`); a device with a finite gate set
approximates them.

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
- **Reverse.** In reversible mode each step appends to an undo journal exactly what it overwrites: the old value
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
- **Quantum state** is a sparse map from basis configuration (the values of all live registers) to a complex
  amplitude. Gates act on amplitudes. This is numerical (f64), and is evidence, not proof.
- **`measure`** samples with a seeded generator and keeps only the configurations that agree with the outcome,
  rescaled. It names a register, or `N` bits at an address (`measure iN [ADDR]`, "Qubits in memory").

### `apply`

`apply @f(args) -> out, fault budget B`. Each argument is an ordinary value or a register read as an integer;
`out` and `fault` are registers (`fault` at least 5 qubits wide) and may not appear among the arguments. The
function `@f` is the **applied function**; some fault and option names call it a *region*.

**Meaning.** For each definite value `a` of the argument registers, run `@f(a)` for exactly `B` steps on a
private copy of memory as it stands, starting from a fresh frame above the caller's:

- if it has returned `r` by then: `out ^= r`, `fault` unchanged;
- if it has stopped with a named fault `c`, or has not finished (`c` is then `budget`): `fault ^= c`, `out`
  unchanged.

Then the private memory is discarded. The arguments are unchanged, so on the arguments and the pair
(`out`, `fault`) `apply` is the unitary that takes `|a, y, z⟩` to `|a, y ⊕ r, z⟩` for the values `a` where `@f`
returns `r`, and to `|a, y, z ⊕ c⟩` where it ends with fault code `c`: a permutation of basis
configurations, which undoes itself when applied again with the same memory and the same `B`. Inside `@f` a
quantum instruction is the fault `quantum_in_region`, `effect` is `effect_in_region` and `exit` is
`exit_in_region`, each an ordinary stop of the run of `@f`. This is the lifted-oracle contract of the North
Star (§4.2).

The executor is written to compute it so, and only so (`crates/exec/src/region.rs`); that it does is a claim
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
is fixed before it starts. That circuit repeats one step circuit, the same each time, `B` times, so nested
loops cost nothing beyond the steps they take.

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
- *The start is one basis state, the same for every argument value*: memory is definite before the `apply`,
  because ordinary code cannot read a qubit. The validator refuses a register where an integer is expected
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

**Faults for some argument values.** With a fault register that starts at 0, measuring it afterwards finds a
fault with probability equal to the total weight of the faulting configurations, and the state collapses onto
the outcome found; nothing in the program learns more than that one measurement shows. The executor sees every
configuration.
It counts each `apply` execution that faulted for some configuration of nonzero amplitude, with their weight,
in the run statistics. By default such an `apply` stops the run with the run error `region fault`, leaving the
state as it was before the `apply`; `allow_region_faults` (`bennett run --allow-region-faults`) lets the run go
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
  ("Overwrites that keep the old value" below).
- **A one-bit update `x = e(x)` with `e(1) == e(0) ^ 1`** is `x ^= e(0)`. The identity is proved by LLVM's
  simplifier on two clones of `e`, or, where `e` reads no ordinary value and the other qubit variables it
  reads have at most `maxEnumeratedSupportBits` (8) bits that may be 1, by folding both clones on every
  assignment of those bits (`Control.cpp`, `provesFlipByCases`): InstCombine rewrites the negated `select` of
  `q && !r` by De Morgan into another `select`, so `(s ^ 1) ^ s` did not fold and `y ^= q && !r` was refused
  until 2026-11-17 (the fallback oracle's `coherent_xor`, and `coherent_nested` at O0).
- **Conditional jumps, joins and loops that depend on a qubit** are outlined into ordinary functions and
  applied. They may not write memory yet: the front refuses such a write (decided, not built: it will keep
  its value in qubits at the bytes it writes; `HANDOFF.md`, "Next steps, in order", 4).
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
  everything it does (or, for several results, a pair of `apply`s that each run all of it), as does a
  function called whole. An update of a qubit variable inside a loop of the control program, whose trip count is
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
  5%) and 5% to 16% more device code (`HANDOFF.md`, "Known weak spots").
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
  no memory and run under qubit control. Runtime calls on constants are evaluated after the optimiser, so a
  rotation's angle may be computed.
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
one budget, whatever loops and calls it contains. With gates it is predicated (above). **Without gates but with
stores into qubit variables** it is also one applied function, for all the variables it writes
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
to 0. Each `apply` runs the whole code: two runs of `B` steps however many results.

Anything else is refused by name: a variable whose value before the code is not known and which the code does
more with than XOR into it would need its old value kept. Without a loop in the code, predication may still
handle such a store (an in-place bijection); with one, the message says the old value would have to be kept,
which is the retained form of a store under a qubit (not built yet: "Next steps" in `HANDOFF.md`). A value the
code leaves for later code that was read from a variable after the code wrote it is refused too, as
predication refuses it. clang `-O2`'s own vectoriser can turn an accumulation into vector lanes, which the front
does not follow; the tests disable it in that loop (`front/tests/c/region_whole.c`).

Acceptance (`front/tests/tests/region_whole.rs`): two nested loops whose trip counts depend on a three-qubit
register `x`, XORing into two variables of values the front does not know and counting into a third, agree
with native C for all eight values of `x` at O0, O2 and with `--taint-all`, every other register back at 0; so
do the parity of `x` accumulated into a `_Bool` and an overwrite in a loop of a variable of known value. The
smallest budget for all values is 128 steps; at 127 the longest runs fault, the others do not. The control
program has no loop left.

**Gate loops.** A loop with gates under a condition that depends on a qubit stays a loop of the control
program, run as written: the circuit model needs a definite list of gates, and the loop's trip count is an
ordinary value written in the C, so the control program issues the gates of every pass, each controlled by
the condition that the code reaches it (`Predicate.h`). That condition is computed once before the loop, by
one `apply` into a fresh qubit (or is one qubit as it is, `if (q)` on a `_Bool`), controls the gate in every
pass, and is computed again after the loop by the same `apply`, which returns the qubit to 0. Nothing is
unrolled or created: the loop is the C's loop, entered where its ordinary guard holds (LLVM may have merged
that guard into the branch on the qubit; the front takes its ordinary part apart again), and ordinary values
it needs from the code before it are computed again before it. It also runs for the configurations whose
path would not reach it, as ordinary code that writes no memory, its gates acting on none of them. For each
definite value of what the condition reads the result is the product of the gates the C applies there.
Refused by name, each with its route:

- a loop whose number of passes depends on a qubit: a circuit is a definite list of gates, so the number of
  passes must be written as an ordinary value, the condition tested inside the loop. Such a loop is a
  different program from `while (x > 0) { gates; x--; }`: it does not consume its loop variable, which keeps
  its value afterwards;
- a loop whose passes decide on values that depend on a qubit carried from pass to pass (`t = x; for (i < n)
  { if (t & 1) h(y); t = step(t); }`): route, one applied function returning a table of control bits for
  all passes, computed before the loop and cleared after it by the same `apply` (not built: a table of one
  bit per pass needs a register as wide as the number of passes, which here is a run-time value, and a
  register's bit is named by a constant; `HANDOFF.md`, "Questions for the maintainer");
- a gate in the loop on a variable the condition reads. The table form is correct when every gate leaves
  what the table function reads unchanged (those registers may be controls; targets elsewhere). A loop
  whose gates change the tested register without ever changing whether the condition holds could later be
  accepted with one control qubit computed once and cleared by computing it again; everything else keeps a
  record of the passes, the retained case of an overwrite (not built);
- a store into a qubit variable in such a loop (not built).

Acceptance (`front/tests/tests/gate_loops.rs`): the front audit's witness `if (q) for (i < n) x(y)`, and a
computed condition with two gate sites, one on odd passes only, give the state computed by hand for n = 0, 1,
2, 3, 6 at O0, O2 and with `--taint-all`, every other register 0; the loop has one controlled gate line.

## Overwrites that keep the old value

An overwrite of a qubit variable whose old value the front can neither XOR away (it is not known), undo by an
inverse (the update is not a bijection it can invert) nor compute back from what is kept (`x = x * x`, `x =
5` on an `x` in superposition, `*y = 0`) is accepted (the maintainer's decision of 2026-11-17). With `n` a
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

This is the form for qubit variables in registers. The same exchange at bytes of memory (a store whose value
or execution depends on a qubit keeps its value in qubits at the bytes it writes), with the clearings that
return `n` to 0 when the old value is known, a function of registers unchanged since, or computed back by an
inverse, is decided and not built (`HANDOFF.md`, "Next steps, in order", 4); such a store is refused by name.

Acceptance: `front/tests/tests/retained.rs` (`x = x * x`, `x += x`, `x = x / 3`, `x = 5`, a conditional
square on a three-qubit `x`: the weights of the values of `x` are those of C, at O0, O2 and with
`--taint-all`; the warning's count); the fallback oracle's `outside_class` (`*y = 0` on a `y` in superposition:
exact basis results, exact Born weights of every preparation, 512 samples, the warning), un-ignored.

## Applied calls

The language rule, in plain words: **a call whose arguments depend on a qubit, to an ordinary function, is
applied whole; the writes it makes are undone when it ends, and only its result survives.** An *ordinary
function* is one of the program's functions that performs no quantum operation, directly or through anything
it can call (`controlFunctions`, `Outline.h`). The rule follows from what `apply` is: for each definite value
of the arguments the function runs on a private copy of memory, its result is XORed into the target, and the
private memory is discarded. Nothing more is needed for a function that writes memory than for one that only
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
  same there (`ControlAnalysis::memoryUnchangedBetween`; refused by name otherwise), and a value used several
  times is computed again for each use, each time from the same memory, so each time with the same result.
- **What the front refuses.** A call whose result is not used, to a function that may write memory: applied,
  it would do nothing at all, while its writes are its only purpose. Two such calls in one computation when
  one may write memory: under the rule each starts from the memory as it was, but in one applied function the
  second would see the first's writes; the route is one register for each call's result, computed and
  cleared around the computation that uses it. A call under a qubit-dependent condition of a function that
  may write memory (outlined, "Conditional jumps, joins and loops" above): its writes natively survive the
  condition.
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
  through other pointers (into the heap, say) and reads through indirect calls are not followed, so the absence
  of the warning proves nothing.
- **`--taint-all`** does not move an ordinary call with ordinary arguments into an `apply`: there its writes
  must survive, so only calls of functions that read memory at most are treated as computable coherently.
  Such a call does not count as one of "two calls that depend on a qubit", and in the applied function it
  runs before anything that depends on a qubit, so it reads memory as it is at its own site, never what an
  applied call in the same computation wrote on the private copy.

The acceptance target of the rule is SQLite (`experiments/sqlite-on-device`, stage 4): C entries that set a
database up with ordinary code and then apply one query whose key depends on a three-qubit register; each of
eight queries gives the native result on each of the eight rows, and memory is unchanged across each `apply`
(`front/tests/tests/sqlite_applied.rs`, ignored by default since it needs the SQLite sources).

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
  `bad_rewind`, both removed on 2026-11-17, and stay unused.
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
  exact. A byte measured in part keeps holding qubits; nothing else gives a byte back (not its qubits
  becoming definite otherwise, nor its stack frame returning). The rule reads no amplitude: a device keeps
  it as a table of addresses, which a measurement result updates classically.
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

How the simulator stores the result (`crates/exec/src/forward.rs`, `quantum.rs`) is its own affair:

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
it off, so the two options now do the same (`HANDOFF.md`, "Questions for the maintainer"). Measured in
`crates/exec/tests/strict.rs`: an `apply` out of budget stops strict mode and gives, seed by seed, what
`allow_region_faults` gives.

## What is promised about coherence

Decided by the maintainer on 2026-11-13, for ordinary code that overwrites values while it depends on qubits
(what it overwrote, kept, is entangled with the data unless the program un-runs it):

1. **Classical results are exact.** If the state was classical before, the C code runs, and you measure, you get
   what the C program says. By linearity the same holds for each definite value of a superposition measured at
   once. For the device program this is what `apply` means ("`apply`"); that the front lowers C to a device
   program with the C program's meaning is the toolchain's own claim, checked against native builds
   ("From C to a device program"), not something the argument for `apply` shows.
2. **Coherence is guaranteed for a documented class of updates**: gates, `^=`, the in-place bijections in
   `docs/MARKERS.md`, and anything the program un-runs with everything it read unchanged since (rule 4). The
   class may grow; it never shrinks.
3. **Everywhere else, how much coherence survives is unspecified.** The toolchain may keep more or less of what
   was overwritten as it sees fit. Leaving it entangled is not a bug and not refused; the toolchain warns.
4. **Un-running code after something changed what it read or wrote** is likewise not refused. The result is
   unspecified and the toolchain warns.

Today ordinary code acts on qubits only through `apply`, which keeps nothing of its run, so nothing overwritten
is left entangled and rules 3 and 4 have nothing to apply to. The one loss of coherence a C program can meet is
the release of a qubit variable whose value the front does not know (the partial trace, "Releasing a
register"); the simulator reports it, the front does not warn.

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
for each in `HANDOFF.md`. Metadata uses a model device/inode identity, root ownership and zero
timestamps, rather than borrowing host-dependent values. This does not implement all Linux filesystem
semantics; allocation failure, permissions, metadata and namespace lifetime debts are recorded there.

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

What is promised: every result that is not a NaN is bit for bit what the native x86-64 build computes; a NaN
result is a NaN. Which NaN, when several operands are NaNs, is unspecified, as in LLVM, whose optimiser commutes
operands, and on the hardware, where it depends on the instruction form. Measured against this machine's SSE
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

The IR mnemonic tests (`crates/ir/tests/mnemonic_contract.rs`) specify all 36 words independently of
the production macro, check each reader variant and writer instruction against that specification,
and reject missing operands. Shared reader/writer swaps cannot hide behind a round trip. The explicit
macro-expansion mutation trials and their failing tests are recorded in HANDOFF's IR/CLI mutation section.

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

The optimiser, a lowering of the device program to a circuit (the executor simulates the model's rules; it
does not compile to gates), any cost beyond step and qubit counts, certificates, target descriptions, a binary
format, memory written under qubit control that outlives the `apply`, and recomputation shared between
conditions (each predicated operation computes and clears its own).

## Open questions

- **Budget unit.** Steps should be counted in something the source fixes (executed instructions of the normalised
  LLVM bitcode), so that a better lowering never changes which inputs fault. How the plugin tells the executor which
  instructions count is undecided.
- **The adjoint of an applied computation that is not its own inverse.** Gates under qubit control and
  bijective in-place updates are done (above) without one.
- **Memory written under qubit control that later code reads.** Decided on 2026-11-17, not built: such a store
  keeps its value in qubits at the bytes it writes, and measuring those bytes makes them ordinary memory again
  (`HANDOFF.md`, "Next steps, in order").
- **Keeping what ordinary code overwrote under a qubit, and un-running it later.** `apply` computes, copies the
  result out and un-runs at once. An update with no known inverse (`x = x * x`) and a conditional boot need the
  kept values to outlive the `apply`. Decided on 2026-11-17, not built: such an overwrite is accepted, the old
  value kept in spare qubits released by `qfree`, with a warning and the count of qubits kept. A narrow static
  form of this has a proof and may be added with its check (`HANDOFF.md`). Gates, allocation and measurement
  inside code that is later un-run are not part of the model: a program wanting them writes the forward and
  inverse gate sequences itself.
- **Channels.** A dirty discard is the partial trace, measurement is described by effects, noise by channels; the
  finished Bennett covers them (maintainer, 2026-11-13). This step has unitaries, measurement and the partial
  trace (`qfree`, since 2026-11-17), the simulator keeping a purification. Nothing in
  the device program should make channels impossible to add.
- **Exact amplitudes** for the permutation tier.
- **Integers wider than 128 bits.** The device program has integers up to 128 bits (decided 2026-11-13: Rust's
  `u128` gives their semantics; splitting them in the front would need a type legaliser, which LLVM has only in
  SelectionDAG). Wider ones are refused by name.
