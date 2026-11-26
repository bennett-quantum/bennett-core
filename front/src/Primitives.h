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

// The declaration rule as the module shows it, checked on the module as clang wrote it. One recognition, by linker
// name, applied to every symbol of the module: a symbol whose linker name is a primitive's must be the header's
// declaration as a whole, property by property of an LLVM function declaration (a declaration, external linkage,
// default visibility and DLL storage, the C calling convention, the header's function type, `noreturn` exactly where
// the header has it, only the target and code-generation attributes clang 18 puts on it, and nothing else an LLVM
// declaration can carry), or it is refused by name; an alias, an ifunc or a variable under such a name is refused.
// Every call of it, and every call through a pointer of its type where the program takes its address, has the
// header's function type and calling convention, `noreturn` only where the header has it, and no attribute that
// lets LLVM remove, merge, move or speculate a call. Other calls through a pointer are not touched. The check
// applies to every module, whether or not its units include the handle header (the C library's and the marker
// programs' too). Also refuses a program that uses (calls or takes the address of) both a marker (docs/MARKERS.md)
// and one of the seven interface functions; a declaration nothing uses is checked and counts as no use. Stops at
// the first violation with "error: ..." and status 2.
void checkPrimitives(const llvm::Module &module);

// The named metadata bennett-cc puts in the bitcode of every unit that passed the unit check (Driver.cpp); llvm-link
// keeps it in a program linked from such units. The front's own behaviour for the handle path (giving primitives
// callable addresses, checkHandleProgram) applies to a program that has it; a program compiled by clang alone, as on
// the marker path, keeps the old layer's behaviour.
constexpr const char *driverMark = "bennett.cc";

// Whether the module calls or takes the address of one of the seven interface functions.
bool usesHandleInterface(const llvm::Module &module);

// For bennett-front: checkPrimitives, then, if the module uses the handle interface, a refusal by name ("not
// handled yet", status 3) at its first interface call: the translation of the calls into device instructions
// waits on the device forms of docs/HANDLES.md, section 14. Then, in a program with the driver's mark, each
// primitive whose address the program takes gets a callable device function, `<name>.address`, internal, whose body is the direct call; every use other
// than a direct call names it, so a call through a pointer reaches the primitive as a direct call does.
void checkHandleProgram(llvm::Module &module);

} // namespace bennett
