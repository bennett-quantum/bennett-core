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

// Computed goto, as LLVM's IndirectBrExpand does it (which needs a target machine): every block whose
// address is taken gets a number, from 1 and unique in the module; `blockaddress` becomes that number
// as a pointer; each `indirectbr` becomes a `switch` on it over its own destinations, whose default is
// `unreachable` (a jump anywhere else is undefined in C, and a named fault here). Label arithmetic
// (`&&a - &&b`) keeps working, since the numbers are ordinary integers.
void lowerIndirectBranches(llvm::Module &module);

// Removes inline assembly whose template is empty: the barrier `asm("" : "+r"(x) :: "memory")` that
// keeps the optimiser from seeing through a value or across memory. Such an asm executes no instruction;
// what remains is how the compiler places its operands. An output then holds its tied input (`=r,0`), an
// indirect output tied to an input receives that input, and a memory output (`=*m`) is left as it was.
// An output that nothing sets, and any other assembly, stay for the emitter to refuse by name. Runs after
// the optimiser, so the barrier has done its work.
void lowerEmptyAssembly(llvm::Module &module);

// Rewrites min/max/abs/funnel shifts/ctpop/ctlz/cttz/bswap and memset/memcpy/memmove into ordinary
// instructions and loops, and deletes intrinsics that have no meaning here (lifetime, debug, assume).
// With `keepBijections`, funnel shifts and byte swaps stay: the front inverts updates made of them
// (InPlace.h), so they are lowered only after that.
void lowerIntrinsics(llvm::Module &module, bool keepBijections = false);

// Vector values (GCC's vector extensions, or what the optimiser made of an array) become their elements,
// with LLVM's Scalarizer, loads, stores and variable element indices included; InstCombine then folds
// what is left of the vectors. Vectors that cross a call or a return stay, for the emitter to refuse.
void scalarise(llvm::Module &module);

// Runs InstCombine and EarlyCSE on one function: LLVM's simplifier, used to prove small identities.
void simplify(llvm::Function &function);

// Runs LLVM's scalar simplifiers on one function, including its CFG: SCCP, InstCombine, SimplifyCFG,
// EarlyCSE and aggressive dead-code elimination, to a fixed point of the pipeline's own choosing.
void simplifyDeep(llvm::Function &function);

} // namespace bennett
