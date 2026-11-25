#include "Primitives.h"

#include "Diagnostic.h"
#include "Markers.h"

#include <llvm/ADT/ArrayRef.h>
#include <llvm/ADT/StringRef.h>
#include <llvm/IR/Attributes.h>
#include <llvm/IR/DerivedTypes.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/InstrTypes.h>
#include <llvm/IR/Module.h>
#include <llvm/Support/raw_ostream.h>

using namespace llvm;

namespace bennett {

namespace {

// The eleven primitives in the order of docs/HANDLES.md, sections 5.3 to 5.6.
constexpr Primitive primitives[] = {
    {"bq_alloc", PrimitiveGroup::Interface, false},
    {"bq_free", PrimitiveGroup::Interface, false},
    {"bq_width", PrimitiveGroup::Interface, false},
    {"bq_measure", PrimitiveGroup::Interface, false},
    {"bq_gate", PrimitiveGroup::Interface, false},
    {"bq_rotate", PrimitiveGroup::Interface, false},
    {"bq_apply", PrimitiveGroup::Interface, false},
    {"__bennett_effect", PrimitiveGroup::Effect, false},
    {"__bennett_exit", PrimitiveGroup::Effect, true},
    {"__bennett_out_of_memory", PrimitiveGroup::Effect, true},
    {"__bennett_bad_argument", PrimitiveGroup::Effect, true},
};

// The linker name of a symbol: LLVM writes an assembler label that must not be decorated with a leading byte 1.
StringRef linkerName(StringRef name) { return name.starts_with("\1") ? name.drop_front() : name; }

std::string printed(const Type *type) {
  std::string text;
  raw_string_ostream out(text);
  type->print(out);
  return out.str();
}

// Attributes that say nothing about what a call does: clang 18 puts them on the header's declarations and their
// calls (the target and code-generation string attributes, `nounwind` and `nobuiltin` at call sites), or they
// state a fact about an operand that the optimiser found (`nonnull`, alignment, dereferenceable bytes). Any other
// attribute (`memory(...)`, `willreturn`, `speculatable`, `nocapture`, `returned`, ...) can let LLVM remove,
// merge, move or speculate a call, so a declaration or call that carries one is refused.
enum class Position { Function, Return, Parameter };

bool harmless(const Attribute &attribute, Position position, bool atCall) {
  if (attribute.isStringAttribute()) {
    return true;
  }
  switch (position) {
  case Position::Function:
    switch (attribute.getKindAsEnum()) {
    case Attribute::NoUnwind: // C code does not unwind
    case Attribute::NoBuiltin:
    case Attribute::NoReturn: // checked against the header separately
      return true;
    default:
      return false;
    }
  case Position::Return:
  case Position::Parameter:
    switch (attribute.getKindAsEnum()) {
    case Attribute::NoUndef:
    case Attribute::SExt:
    case Attribute::ZExt:
      return true;
    case Attribute::NonNull:
    case Attribute::Alignment:
    case Attribute::Dereferenceable:
    case Attribute::DereferenceableOrNull:
      return atCall;
    default:
      return false;
    }
  }
  return false;
}

// The first attribute of `list` that is not harmless, as text, or empty.
std::string firstHarmful(const AttributeList &list, unsigned params, bool atCall) {
  for (const Attribute &attribute : list.getFnAttrs()) {
    if (!harmless(attribute, Position::Function, atCall)) {
      return attribute.getAsString();
    }
  }
  for (const Attribute &attribute : list.getRetAttrs()) {
    if (!harmless(attribute, Position::Return, atCall)) {
      return attribute.getAsString() + " on the result";
    }
  }
  for (unsigned i = 0; i < params; ++i) {
    for (const Attribute &attribute : list.getParamAttrs(i)) {
      if (!harmless(attribute, Position::Parameter, atCall)) {
        return attribute.getAsString() + " on parameter " + std::to_string(i + 1);
      }
    }
  }
  return "";
}

constexpr const char *route = "; the header bq.h, included by force in every unit, has the only declaration of each "
                              "primitive (docs/HANDLES.md, section 4)";

void checkPrimitiveFunction(const Function &function, const Primitive &primitive) {
  std::string name = "@" + std::string(primitive.name);
  if (!function.isDeclaration()) {
    misuse("the program defines the primitive " + name + route, &function);
  }
  if (!function.hasExternalLinkage()) {
    misuse("the primitive " + name + " is declared weak or with other than external linkage" + route, &function);
  }
  FunctionType *expected = primitiveType(primitive, function.getContext());
  if (function.getFunctionType() != expected) {
    misuse("the primitive " + name + " is declared with the type " + printed(function.getFunctionType()) +
               " where the header's is " + printed(expected) + route,
           &function);
  }
  if (function.doesNotReturn() != primitive.noreturn) {
    misuse(std::string("the primitive ") + name + (primitive.noreturn ? " is declared without" : " is declared with") +
               " `noreturn`" + route,
           &function);
  }
  if (std::string bad = firstHarmful(function.getAttributes(), expected->getNumParams(), false); !bad.empty()) {
    misuse("the primitive " + name + " is declared with `" + bad +
               "`, one of the attributes that let LLVM remove or move its calls" + route,
           &function);
  }
  for (const Use &use : function.uses()) {
    const auto *call = dyn_cast<CallBase>(use.getUser());
    if (!call || !call->isCallee(&use)) {
      continue; // its address, taken: a call through it is the same primitive call
    }
    if (call->getFunctionType() != expected) {
      misuse("a call of the primitive " + name + " through the type " + printed(call->getFunctionType()) +
                 " where the header's is " + printed(expected) + route,
             call);
    }
    if (call->doesNotReturn() && !primitive.noreturn) {
      misuse("a call of the primitive " + name + " is marked `noreturn`" + route, call);
    }
    if (std::string bad = firstHarmful(call->getAttributes(), expected->getNumParams(), true); !bad.empty()) {
      misuse("a call of the primitive " + name + " carries `" + bad +
                 "`, one of the attributes that let LLVM remove or move its calls" + route,
             call);
    }
  }
}

} // namespace

ArrayRef<Primitive> allPrimitives() { return primitives; }

std::optional<Primitive> primitiveNamed(StringRef name) {
  for (const Primitive &primitive : primitives) {
    if (name == primitive.name) {
      return primitive;
    }
  }
  return std::nullopt;
}

FunctionType *primitiveType(const Primitive &primitive, LLVMContext &context) {
  Type *v = Type::getVoidTy(context);
  Type *i8 = Type::getInt8Ty(context);
  Type *i32 = Type::getInt32Ty(context);
  Type *i64 = Type::getInt64Ty(context);
  Type *f64 = Type::getDoubleTy(context);
  Type *ptr = PointerType::get(context, 0);
  StringRef name = primitive.name;
  if (name == "bq_alloc" || name == "bq_width") {
    return FunctionType::get(i64, {i64}, false);
  }
  if (name == "bq_free") {
    return FunctionType::get(v, {i64}, false);
  }
  if (name == "bq_measure") {
    return FunctionType::get(i64, {i64, i64, i64}, false);
  }
  if (name == "bq_gate") {
    return FunctionType::get(v, {i64, i64, i64, ptr, i64}, false);
  }
  if (name == "bq_rotate") {
    return FunctionType::get(v, {i64, f64, i64, i64, ptr, i64}, false);
  }
  if (name == "bq_apply") {
    return FunctionType::get(v, {ptr, i64, i64, ptr, i64, ptr, i64, ptr, i64}, false);
  }
  if (name == "__bennett_effect") {
    return FunctionType::get(v, {i8}, false);
  }
  if (name == "__bennett_exit") {
    return FunctionType::get(v, {i32}, false);
  }
  if (name == "__bennett_out_of_memory" || name == "__bennett_bad_argument") {
    return FunctionType::get(v, {}, false);
  }
  fail("internal: no type for the primitive @" + name.str());
}

void checkPrimitives(const Module &module) {
  for (const GlobalValue &global : module.global_values()) {
    StringRef name = global.getName();
    StringRef linker = linkerName(name);
    std::optional<Primitive> primitive = primitiveNamed(linker);
    if (!primitive) {
      continue;
    }
    if (name != linker) {
      misuse("the program gives a symbol the linker name of the primitive @" + linker.str() +
                 " with an assembler label" + route,
             &global);
    }
    if (isa<GlobalAlias>(global) || isa<GlobalIFunc>(global)) {
      fail("the program defines an alias named after the primitive @" + linker.str() + route);
    }
    if (isa<GlobalVariable>(global)) {
      misuse("the program has a variable with the linker name of the primitive @" + linker.str() + route, &global);
    }
    checkPrimitiveFunction(cast<Function>(global), *primitive);
  }
  const Function *marker = nullptr;
  const Function *handle = nullptr;
  for (const Function &function : module) {
    if (!marker && markerOf(&function)) {
      marker = &function;
    }
    std::optional<Primitive> primitive = primitiveNamed(function.getName());
    if (!handle && primitive && primitive->group == PrimitiveGroup::Interface) {
      handle = &function;
    }
  }
  if (marker && handle) {
    misuse("the program uses both the marker interface (@" + marker->getName().str() +
               ", docs/MARKERS.md) and the handle interface (@" + handle->getName().str() +
               ", docs/HANDLES.md); write it in one of them",
           handle);
  }
}

bool usesHandleInterface(const Module &module) {
  for (const Function &function : module) {
    std::optional<Primitive> primitive = primitiveNamed(function.getName());
    if (primitive && primitive->group == PrimitiveGroup::Interface) {
      return true;
    }
  }
  return false;
}

void checkHandleProgram(const Module &module) {
  checkPrimitives(module);
  for (const Function &function : module) {
    std::optional<Primitive> primitive = primitiveNamed(function.getName());
    if (!primitive || primitive->group != PrimitiveGroup::Interface) {
      continue;
    }
    const Value *at = &function;
    for (const User *user : function.users()) {
      if (isa<CallBase>(user)) {
        at = user;
        break;
      }
    }
    notHandled("the handle interface's @" + function.getName().str() +
                   ": translating its calls into device instructions waits on the device forms of "
                   "docs/HANDLES.md, section 14",
               at);
  }
}

} // namespace bennett
