// Predication: quantum operations under a branch on a value that depends on a qubit.
//
// On each basis configuration the region runs one path, and the quantum operations on that path act in
// path order. The region is replaced by a straight sequence of the same operations, in an order that is
// consistent with every path (a topological order of the region), each acting only on the configurations
// whose path reaches it:
//
// - a gate becomes a gate controlled by the condition "this configuration reaches the gate";
// - a store into a qubit variable x becomes `x ^= d`, where d is the store's change to x on the
//   configurations that reach it and 0 on the others; or, when that change depends on x itself, the
//   store of "the new value where reached, the old one elsewhere", which LLVM must turn into straight-line
//   code and the in-place update (InPlace.h) must invert.
//
// Each condition and each change is an ordinary function: a copy of the region that returns the
// operation's value where the operation is reached and the default at the region's exit. The copy keeps
// the region's control flow, so nothing is computed on a configuration whose path would not compute it.
// Its qubit variables are read where the operation is; a value read in the region before another
// operation changed that variable, which the copy would need, is refused by name.
#pragma once

#include <vector>

namespace llvm {
class BasicBlock;
class Function;
} // namespace llvm

namespace bennett {

class ControlAnalysis;

// Whether `blocks` contain a gate or a store into a qubit variable.
bool hasQuantumOperations(const std::vector<llvm::BasicBlock *> &blocks, const ControlAnalysis &analysis);

// Moves every quantum operation of the region `blocks` (entry first; its edges out of the region all go
// to `exit`) after it, as described above. The region is left without them. Refuses by name what it
// cannot do: a loop in the region, a measurement, a gate whose condition reads its own qubit variable.
void predicateRegion(llvm::Function &function, const std::vector<llvm::BasicBlock *> &blocks,
                     llvm::BasicBlock *exit, ControlAnalysis &analysis);

} // namespace bennett
