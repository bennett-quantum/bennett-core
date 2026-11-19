// Every LLVM pass the front runs, and the instructions those passes may not see into.
//
// Before LLVM's optimiser the front decides, on the program as clang hands it over, which instructions the
// language's own rules depend on (Classify.h) and marks each with the metadata `bennett.class`. Every later run of
// an LLVM pass that can move, merge, copy or remove an instruction (the optimiser, the Scalarizer and InstCombine
// that split vector values, the simplifier runs on code the front made) goes through this module, which
// *seals* the marked instructions for the run: each becomes a call of a declaration of its own, without a body
// and with the one attribute `noduplicate`. A pass must assume such a call reads and writes any memory, and may
// not return, so it keeps the call where it is, under its conditions and in order with every other access to
// memory; it never merges two calls of different functions, and it may not copy one (loop rotation copies a
// loop's first block otherwise). After the run each call becomes the instruction it stood for
// again (type, alignment, volatility, atomic ordering, attributes, metadata). Then the front checks that every
// sealed instruction is there exactly once: one that a pass removed or copied is refused by name.
//
// The argument rests on one property of LLVM's passes (a call of an unknown function is left as it stands), not
// on a list of their steps. Passes that only infer attributes or delete unreferenced globals see the real
// instructions: sealed, the attributes they infer would be too weak.
#pragma once

namespace llvm {
class Function;
class Module;
} // namespace llvm

namespace bennett {

// LLVM's O2 pipeline without the vectorisers (the device program has no vector values), with the classified
// instructions sealed. Functions that came from `clang -O0` lose `optnone` and the `noinline` that comes with
// it first: the front end, not the user's flags, fixes the pipeline. A crash inside LLVM is refused by name.
void optimiseSealed(llvm::Module &module);

// SROA on every function: local variables whose address never escapes become values. It rewrites only the
// accesses to those variables; it moves, merges and removes no other load, store or call. Runs before the
// classification, which then never sees a local variable that is memory only at -O0.
void promoteLocals(llvm::Module &module);

// Infers the memory effects and other attributes of every function again, from its real instructions.
void inferAttributes(llvm::Module &module);

// Deletes globals and functions nothing refers to, then infers attributes again.
void removeDeadGlobals(llvm::Module &module);

// One round of the Scalarizer (vector loads, stores and variable element indices included), InstCombine and
// dead-code elimination over every function, with the classified instructions sealed. Classified vector loads
// and stores must have been split before (Classify.h): sealed, the Scalarizer cannot split them.
void scalariseSealed(llvm::Module &module);

// InstCombine and EarlyCSE on one function, sealed: LLVM's simplifier, used to prove small identities.
void simplify(llvm::Function &function);

// LLVM's scalar simplifiers on one function, sealed, including its CFG: SCCP, InstCombine, SimplifyCFG, EarlyCSE
// and aggressive dead-code elimination, to a fixed point of the pipeline's own choosing.
void simplifyDeep(llvm::Function &function);

} // namespace bennett
