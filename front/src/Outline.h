// Outlining: a branch, switch or phi that depends on a qubit cannot run in the control program, so the
// smallest single-entry single-exit region around it is outlined with LLVM's CodeExtractor into an ordinary
// function, which the control program then applies coherently. CodeExtractor returns outputs through
// pointers; each output instead becomes its own function returning that value (the region recomputed per
// output: a cost, never a correctness question).
#pragma once

#include <set>

namespace llvm {
class Function;
class Module;
} // namespace llvm

namespace bennett {

// The functions that perform quantum operations: those that call a marker or such a function.
std::set<const llvm::Function *> controlFunctions(const llvm::Module &module);

// Outlines every region of `function` that branches or merges on a value that depends on a qubit.
void outlineRegions(llvm::Function &function, bool taintAll, const std::set<const llvm::Function *> &control);

} // namespace bennett
