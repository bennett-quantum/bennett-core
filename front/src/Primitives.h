// The primitives of the handle interface (docs/HANDLES.md, section 4): the seven interface functions of `bq.h`
// and the four reserved effect primitives, with the LLVM function types clang 18 gives their header
// declarations on the device's target, and the check of the declaration rule on the module the front reads.
//
// Nothing here asks what a value depends on. A primitive is known by its linker name alone, and the checks
// look at declarations, their attributes and the function type of each call, never at operands.
#pragma once

#include <llvm/ADT/ArrayRef.h>

#include <optional>
#include <string>

namespace llvm {
class CallBase;
class Function;
class FunctionType;
class LLVMContext;
class Module;
class StringRef;
} // namespace llvm

namespace bennett {

enum class PrimitiveGroup {
  Interface, // bq_alloc, bq_free, bq_width, bq_measure, bq_gate, bq_rotate, bq_apply
  Effect,    // __bennett_effect, __bennett_exit, __bennett_out_of_memory, __bennett_bad_argument
};

struct Primitive {
  const char *name; // the C identifier, which is also the linker name
  PrimitiveGroup group;
  bool noreturn; // declared _Noreturn by the header
};

// The primitive whose linker name is `name`, if any. A name with LLVM's leading byte 1 (an assembler label
// written as "\x01name") is not a primitive's: checkPrimitives refuses it.
std::optional<Primitive> primitiveNamed(llvm::StringRef name);

// Every primitive, in the order of docs/HANDLES.md, sections 5.3 to 5.6.
llvm::ArrayRef<Primitive> allPrimitives();

// The LLVM type of the header's declaration of `primitive`, as clang 18 emits it for the device's target:
// a handle `bq_reg` is passed and returned as `i64`, a pointer as `ptr`, `char` as `i8`, `int` as `i32`.
llvm::FunctionType *primitiveType(const Primitive &primitive, llvm::LLVMContext &context);

// The declaration rule as the module shows it, checked on the module as clang wrote it: every symbol whose
// linker name is a primitive's is refused by name unless it is a declaration of a
// function with external linkage, the header's function type, no attribute that lets LLVM remove, merge, move
// or speculate a call (only the target and code-generation attributes clang 18 puts on the header's
// declarations, `noreturn` exactly where the header has it), and every call of it has that function type and
// such attributes. A symbol that is an alias or an ifunc is refused. Where the program takes a primitive's
// address other than as a direct callee, a call through a pointer may reach it, so no call through a pointer may
// carry such an attribute either. The check applies to every module, whether or not its units include the handle
// header (the C library's and the marker programs' too). Also refuses a program that uses both the marker interface (docs/MARKERS.md) and one of the
// seven interface functions. Stops at the first violation with "error: ..." and status 2.
void checkPrimitives(const llvm::Module &module);

// Whether the module refers to one of the seven interface functions.
bool usesHandleInterface(const llvm::Module &module);

// For bennett-front: checkPrimitives, then, if the module uses the handle interface, a refusal by name ("not
// handled yet", status 3) at its first interface call: the translation of the calls into device instructions
// waits on the device forms of docs/HANDLES.md, section 14.
void checkHandleProgram(const llvm::Module &module);

} // namespace bennett
