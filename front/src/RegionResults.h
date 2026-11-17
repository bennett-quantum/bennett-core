// Code without gates under a condition that depends on a qubit, which writes qubit variables: one applied
// function for all of its results (docs/DESIGN.md, "Code under a qubit").
//
// The region (outlined around a qubit-dependent branch or join, Outline.h) runs, for each definite value of
// what it reads, on private memory, loops, nested loops and calls included, under one budget. For each qubit
// variable y it writes, the applied function returns `final XOR start`: the variable's value at the region's
// exit XOR its value at the entry, where in the private run y starts at
//
// - its known value, when the variable holds an ordinary value before the region (fresh, assigned an ordinary
//   value, or measured): the region may read y freely; or
// - 0, when every value the region computes from y is `y XOR d` for a d that does not read y (y is only
//   accumulated into by XOR): then `final(y) = y XOR final(0)`.
//
// One result is XORed into its variable by one `apply`. Several share a fresh register N: `apply` into N, a
// CNOT from each bit of N into its variable, the same `apply` again (its arguments are unchanged, so N returns
// to 0), release N. Nothing is created that the C does not contain: no loop, counter or bound; the region's
// loops stay inside the applied function.
#pragma once

#include <vector>

namespace llvm {
class BasicBlock;
class Function;
} // namespace llvm

namespace bennett {

class ControlAnalysis;

// Whether `blocks` contain a gate.
bool hasGates(const std::vector<llvm::BasicBlock *> &blocks);

// Replaces the region `blocks` (entry first; its edges out of the region all go to `exit`), which contains no
// gate and stores into qubit variables, as described above, and returns true. Returns false, changing
// nothing, when a variable the region writes is neither known before it nor only accumulated into by XOR and
// the region has no cycle (predication may still handle it); refuses by name in the same case when the region
// has a cycle.
bool applyRegionWhole(llvm::Function &function, const std::vector<llvm::BasicBlock *> &blocks, llvm::BasicBlock *exit,
                      ControlAnalysis &analysis);

} // namespace bennett
