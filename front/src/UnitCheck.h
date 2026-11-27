// The declaration check of one unit before any optimiser pass (docs/DESIGN.md, "The declaration check"; rules 2 to
// 4 there), run by bennett-cc on the unit's LLVM IR as clang wrote it with every pass disabled, against the
// *baseline*: the same compilation of a unit that only calls each primitive once.
//
// clang emits a declaration where something first uses it and fixes its symbol at that use. The forced include
// bq-references.h makes the first use of every header declaration an internal array of the eleven primitives'
// addresses, ahead of the unit's own text, so the array's entries show the symbols the header's declarations stand
// for at the end of the unit. Then:
// - rule 2: entry k is the only global whose linker name is primitive k's, and it is a function declaration under
//   that name (Primitives.h, checkRedirection, refuses the other kinds);
// - rule 3: that declaration is the baseline's, as the LLVM 18 assembly writer prints it, with the attribute group
//   written out and `!dbg` left out;
// - rule 4: each direct call of it has the declaration's type and calling convention, the baseline call's result
//   and parameter attributes, the baseline call's function attributes plus at most the context attributes of
//   Claims.h, metadata only `!dbg` and `!llvm.access.group`, and no operand bundle.
// A unit given as LLVM IR has no array: entry k is then the module's global named after the primitive, if any.
#pragma once

namespace llvm {
class Module;
} // namespace llvm

namespace bennett {

// The name of the array bq-references.h defines.
constexpr const char *referencesName = "__bennett_references_";

// Rules 2 to 4 on `unit` against `baseline` (both in one LLVMContext, so that equal attribute sets and uniqued
// metadata are the same objects). `withReferences`: the unit was compiled from C with the forced includes and must
// have the array. Stops at the first violation with "error: ..." and status 2. The declarations are printed
// temporarily without their function attributes and metadata, so both modules are written to and restored.
void checkUnit(llvm::Module &unit, llvm::Module &baseline, bool withReferences);

// Deletes the reference array and the primitive declarations nothing else uses (the array alone made clang emit
// them), so that the unit is what clang writes without the forced include of the array.
void removeReferences(llvm::Module &unit);

} // namespace bennett
