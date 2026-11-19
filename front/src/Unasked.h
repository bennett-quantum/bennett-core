// Ordinary code the control program runs for inputs on which C does not run it.
//
// A loop with gates under a condition that depends on a qubit stays a loop of the control program (Predicate.h):
// a circuit is a definite list of gates, so the loop's ordinary code (its counter, its tests, what it computes
// from values before it) runs for every input that reaches it in the control program, also for the
// configurations whose path in C would not enter the loop. Such an instruction runs *unasked*, and is allowed
// only if it cannot fault there and changes nothing later code can see (docs/DESIGN.md, "Code under a qubit",
// gate loops). This is an allow-list: what is not on it is refused by name.
//
// - It cannot fault: proved where the device runs it, for every input that reaches it there, using only
//   conditions the device itself tests there. Integer arithmetic other than division, comparisons, choices,
//   casts and address arithmetic never fault on the device (a shift by too much gives 0, signed overflow wraps).
//   A division or remainder needs a divisor that is a nonzero constant, has a constant bit set, or is tested
//   nonzero by a conditional jump of the loop itself on every path to the division. A read of memory, a
//   reservation of stack and a trap are refused.
// - It changes nothing later code can see: a store, a call (other than the bit-numbering marker, the
//   bijections the front keeps as intrinsics, and intrinsics that do nothing on the device) are refused.
//
// Nothing is proved about the loop's number of passes: the loop is issued in full whatever the qubits hold, and
// a loop that does not end once entered does not end on the device (the maintainer's decision of 2026-11-19).
// Where LLVM's loop analysis finds no count, the front warns.
#pragma once

namespace llvm {
class Instruction;
class Loop;
class DominatorTree;
} // namespace llvm

namespace bennett {

// Refuses by name `inst` unless it may run unasked. `loop` is the gate loop that contains it, whose own
// conditional jumps may prove a divisor nonzero, or null for code computed before the loop (only constants
// prove a divisor there). `dominators` is the dominator tree of the function as it is.
void requireRunsUnasked(const llvm::Instruction &inst, const llvm::Loop *loop, const llvm::DominatorTree &dominators);

// Warns at `at` when LLVM's loop analysis (ScalarEvolution), run on the function as it is, finds no count of
// the passes of `loop`.
void warnUncountedGateLoop(const llvm::Loop &loop, const llvm::Instruction &at);

} // namespace bennett
