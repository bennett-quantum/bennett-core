# Design note

A working note. It records what is decided for now and what is left open. Nothing here is a contract; change it when
the code teaches us something, and say why in the commit.

## Principles

1. **Centre outward.** The first program that must run end to end is the thesis itself: rotate a bit, let ordinary
   code depend on it, measure. Everything grows by making that ordinary code more real.
2. **Correct first, cheap second.** Where the toolchain does not know something it takes the expensive correct route
   and reports the cost. It never refuses ordinary code and never needs an exact analysis to be correct. Misuse of a marker may be
   refused, with a clear error.
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

- **Values** are integers of any width from 1 to 64 bits. A pointer is an integer address into one flat byte memory.
  There are no floating-point values: floating point arrives as library code.
- **Functions** have typed parameters and at most one result. Bodies are basic blocks in SSA form with block
  parameters. Every name is resolved to a dense index when the module is loaded; nothing is looked up by name at
  run time.
- **Classical instructions:** constants; `add sub mul udiv sdiv urem srem and or xor shl lshr ashr`; `icmp`;
  `select`; `zext sext trunc`; `load` and `store` at a width, little-endian; `alloca` (frame lifetime); direct `call`;
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

## Not in this step

The plugin, the optimiser, any cost beyond step and qubit counts, certificates, target descriptions, a binary
format, in-place bijections, coherent memory that outlives a region, and quantum instructions inside an applied
function.

## Open questions

- **Budget unit.** Steps should be counted in something the source fixes (executed instructions of the normalised
  LLVM bitcode), so that a better lowering never changes which inputs fault. How the plugin tells the executor which
  instructions count is undecided.
- **Quantum actions inside regions** (rotations under control, `x += a` in place) and the adjoint of a region that
  is not its own inverse.
- **Coherent memory across regions.**
- **Running code under a qubit with its history kept.** `apply` is the cleaned-up special case: compute, copy the
  result out, uncompute at once. The general case keeps the journal as quantum workspace and un-runs later. That is
  what `when (q) x += a`, a controlled region that contains gates, and a conditional boot all need. The executor has
  both halves (journal, superposition) but no instruction joins them.
- **Channels.** A dirty discard is the partial trace, measurement is described by effects, noise by channels; the
  finished Bennett covers them (maintainer, 2026-11-13). This step is unitaries and clean discard, and a dirty
  `qfree` is a fault. Nothing in the device program should make channels impossible to add.
- **How `front` (C++) produces `ir`:** a text writer, or a builder exported from the Rust crate.
- **Integers wider than 64 bits**, and exact amplitudes for the permutation tier.
