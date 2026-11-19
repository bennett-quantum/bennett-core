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
//   conditions the device itself tests there. Integer arithmetic other than division, comparisons, choices, casts,
//   address arithmetic and funnel shifts never fault on the device (a shift by too much gives 0, signed overflow
//   wraps). A division or remainder needs a divisor whose range, computed with the device's wrapping arithmetic
//   from constants, excludes 0, or that a conditional jump of the control program tests nonzero on every path
//   to the division. A read of memory, a reservation of stack and a trap are refused.
// - It changes nothing later code can see: a store, a call (other than the bit-numbering marker, the funnel
//   shifts and byte swaps the front keeps as intrinsics until after outlining, and intrinsics that do nothing
//   on the device) are refused.
//
// Nothing is proved about the loop's number of passes: the loop is issued in full whatever the qubits hold, and
// a loop that does not end once entered does not end on the device (the maintainer's decision of 2026-11-19).
// Where LLVM's loop analysis finds no count for the loop or a loop nested in it, the front warns there.
#pragma once

namespace llvm {
class BasicBlock;
class Instruction;
class Loop;
class DominatorTree;
} // namespace llvm

#include <set>

namespace bennett {

// Refuses by name `inst` unless it may run unasked where it now is. `dominators` is the dominator tree of the
// function as it is; a conditional jump in `untested` (code under a qubit, which becomes applied code) is not one
// the control program makes, and proves nothing.
void requireRunsUnasked(const llvm::Instruction &inst, const llvm::DominatorTree &dominators,
                        const std::set<const llvm::BasicBlock *> &untested);

// Warns at each loop of `loop`'s nest, `loop` included, for which LLVM's loop analysis (ScalarEvolution), run on
// the function as it is, finds no count of passes.
void warnUncountedGateLoop(const llvm::Loop &loop);

} // namespace bennett
