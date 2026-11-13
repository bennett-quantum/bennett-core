// Normalisation before analysis: LLVM's own O2 pipeline, then the intrinsics the device program has no
// instruction for are rewritten into ordinary instructions with LLVM's own utilities where they exist.
#pragma once

#include <llvm/IR/PassManager.h>

namespace llvm {
class Function;
class Module;
class PassBuilder;
} // namespace llvm

namespace bennett {

// Runs the O2 pipeline without the vectorisers (the device program has no vector values). Functions that
// came from `clang -O0` lose `optnone` and the `noinline` that comes with it first: the front end, not the
// user's flags, fixes the pipeline.
void optimise(llvm::Module &module, llvm::PassBuilder &builder, llvm::ModuleAnalysisManager &analyses);

// Gives every `byval` parameter its private copy explicitly: the callee copies the pointee into an alloca
// of its own on entry and uses that, and the attribute is dropped from the function and its call sites.
// Refuses parameters whose meaning the device program cannot give (`inalloca`, `preallocated`,
// `swifterror`). `sret` and `byref` mean nothing beyond a pointer, so they need nothing. Idempotent.
void lowerParameterAttributes(llvm::Module &module);

// Rewrites min/max/abs/funnel shifts/ctpop/ctlz/cttz/bswap and memset/memcpy/memmove into ordinary
// instructions and loops, and deletes intrinsics that have no meaning here (lifetime, debug, assume).
void lowerIntrinsics(llvm::Module &module);

// Runs InstCombine and EarlyCSE on one function: LLVM's simplifier, used to prove small identities.
void simplify(llvm::Function &function);

} // namespace bennett
