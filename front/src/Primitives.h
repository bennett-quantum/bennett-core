// The primitives of the handle interface (docs/HANDLES.md, section 4): the seven interface functions of `bq.h` and
// the four reserved effect primitives, known by their linker names, and the checks that hold on every module after
// optimisation (docs/DESIGN.md, "The declaration check"): no other global carries a primitive's linker name, and
// each call of a primitive's declaration has that declaration's type and calling convention.
//
// Nothing here asks what a value depends on. A primitive is known by its linker name alone, and the checks look at
// globals, declarations and the type of each call, never at operands.
#pragma once

#include <llvm/ADT/ArrayRef.h>

#include <optional>
#include <string>

namespace llvm {
class Function;
class GlobalValue;
class Module;
class StringRef;
class Type;
} // namespace llvm

namespace bennett {

enum class PrimitiveGroup {
  Interface, // bq_alloc, bq_free, bq_width, bq_measure, bq_gate, bq_rotate, bq_apply
  Effect,    // __bennett_effect, __bennett_exit, __bennett_out_of_memory, __bennett_bad_argument
};

struct Primitive {
  const char *name; // the C identifier, which is also the linker name
  PrimitiveGroup group;
};

// Every primitive, in the order of docs/HANDLES.md, sections 5.3 to 5.6. The forced include bq-references.h lists
// their addresses in this order.
llvm::ArrayRef<Primitive> allPrimitives();

// The primitive whose C identifier is `name`, if any (a module's IR name, not its linker name).
std::optional<Primitive> primitiveNamed(llvm::StringRef name);

// The symbol the linker sees for `global`: its IR name with a leading byte 1 (an assembler label) removed and the
// target's global prefix added, as LLVM's Mangler computes it.
std::string linkerName(const llvm::GlobalValue &global);

// The linker name a declaration named `name` has in `module`: the name with the target's global prefix.
std::string linkerNameOf(llvm::StringRef name, const llvm::Module &module);

// A type as the LLVM assembly writer prints it.
std::string printed(const llvm::Type *type);

// A calling convention as the LLVM assembly writer spells the common ones, else `cc N`.
std::string conventionName(unsigned convention);

// The phrase the refusals end with: where the only declaration of each primitive is.
extern const char *const declarationRoute;

// Refuses by name (status 2) any global whose linker name is a primitive's and that is not a plain function
// declaration under that IR name: a definition, an alias, an ifunc, a variable, or a global named by an assembler
// label (a leading byte 1). Rule 2 of the check by kind; bennett-cc runs it before and after optimisation and
// bennett-front on every module it reads.
void checkRedirection(const llvm::Module &module);

// The shape after optimisation: the declaration of each primitive the module has keeps external linkage and the C
// calling convention, and every direct call of it has its function type and calling convention. Refused by name
// (status 2) otherwise.
void checkCallShape(const llvm::Module &module);

// The named metadata bennett-cc puts in the bitcode of every unit it compiled (Driver.cpp); llvm-link keeps it in a
// program linked from such units.
constexpr const char *driverMark = "bennett.cc";

// Whether the module calls or takes the address of one of the seven interface functions.
bool usesHandleInterface(const llvm::Module &module);

} // namespace bennett
