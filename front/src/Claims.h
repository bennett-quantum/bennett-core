// What an attribute or a metadata attachment on a call tells LLVM, for the declaration check of the handle
// interface (docs/DESIGN.md, "The declaration check"), and the removal of claims from calls through a pointer.
//
// An attribute or a metadata attachment only gives the optimiser permission to do something (delete, merge, move or
// speculate a call, assume what it does to memory or returns), or limits it, or says how arguments are passed.
// Removing a permission never changes what a defined program does; it only costs optimisation. So before any pass,
// every call whose callee is not a function loses every attribute that is neither ABI, nor a limit on the
// optimiser, nor `noreturn`/`nounwind`, and every metadata attachment other than `!dbg`: a call through a pointer
// that reaches a primitive then runs, whatever the pointer's declaration claimed.
//
// Each attribute kind of LLVM 18 is classified once, in one switch with no default (Claims.cpp, compiled with
// -Werror=switch), so a kind a later LLVM adds stops the build until it is classified; the same for LLVM's fixed
// metadata kinds. String attributes and metadata kinds registered by name are permissions and are removed.
#pragma once

namespace llvm {
class CallBase;
class Module;
} // namespace llvm

namespace bennett {

// Removes the claims from every call in `module` whose callee is not a function (a pointer, inline assembly, an
// alias or an ifunc). Returns how many calls changed.
unsigned removePointerCallClaims(llvm::Module &module);

// For a direct call of a primitive: whether attribute kind `kind` may appear on the call beyond the attributes of
// the header's own call. These come from a statement's context, not from a declaration: `#pragma STDC FENV_ACCESS`
// (`strictfp`), `[[clang::nomerge]]`, `[[clang::noinline]]` and `[[clang::always_inline]]`. They limit what the
// optimiser does with the caller and say nothing about the callee.
bool contextAttributeOnDirectCall(unsigned kind);

// For a direct call of a primitive: whether metadata kind `kind` (an ID of the call's LLVMContext) may be attached:
// `!dbg`, and `!llvm.access.group`, which clang writes for a loop pragma (the program's claim about its own loop).
bool metadataOnDirectCall(unsigned kind);

} // namespace bennett
