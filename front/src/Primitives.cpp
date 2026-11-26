#include "Primitives.h"

#include "Diagnostic.h"
#include "Markers.h"

#include <llvm/ADT/ArrayRef.h>
#include <llvm/ADT/StringRef.h>
#include <llvm/IR/Attributes.h>
#include <llvm/IR/DerivedTypes.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InstrTypes.h>
#include <llvm/IR/Module.h>
#include <llvm/Support/raw_ostream.h>

#include <vector>

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

std::string conventionName(CallingConv::ID convention) {
  switch (convention) {
  case CallingConv::Fast:
    return "fastcc";
  case CallingConv::Cold:
    return "coldcc";
  case CallingConv::Win64:
    return "win64cc (ms_abi)";
  default:
    return "cc " + std::to_string(convention);
  }
}

// The checks of one call that may reach `primitive`: a direct call, or a call through a pointer of the primitive's
// type in a program that takes its address. `subject` names the call, for the message.
void checkCall(const CallBase &call, const Primitive &primitive, const std::string &subject) {
  FunctionType *expected = primitiveType(primitive, call.getContext());
  if (call.getFunctionType() != expected) {
    misuse(subject + " through the type " + printed(call.getFunctionType()) + " where the header's is " +
               printed(expected) + route,
           &call);
  }
  if (call.getCallingConv() != CallingConv::C) {
    misuse(subject + " uses the calling convention " + conventionName(call.getCallingConv()) +
               " where the header's is C's" + route,
           &call);
  }
  if (call.doesNotReturn() && !primitive.noreturn) {
    misuse(subject + " is marked `noreturn`" + route, &call);
  }
  if (std::string bad = firstHarmful(call.getAttributes(), expected->getNumParams(), true); !bad.empty()) {
    misuse(subject + " has attributes that let LLVM remove or move its calls (`" + bad + "`)" + route, &call);
  }
}

// The header's declaration as a whole: a symbol with a primitive's linker name must be exactly what clang 18
// writes for the header's declaration, property by property of an LLVM function declaration.
void checkPrimitiveFunction(const Function &function, const Primitive &primitive) {
  std::string name = "@" + std::string(primitive.name);
  if (!function.isDeclaration()) {
    misuse("the program defines the primitive " + name + route, &function);
  }
  if (!function.hasExternalLinkage()) {
    misuse("the primitive " + name + " is declared weak or with other than external linkage" + route, &function);
  }
  if (function.getVisibility() != GlobalValue::DefaultVisibility ||
      function.getDLLStorageClass() != GlobalValue::DefaultStorageClass) {
    misuse("the primitive " + name + " is declared with a visibility or DLL storage other than the default" + route,
           &function);
  }
  if (function.getCallingConv() != CallingConv::C) {
    misuse("the primitive " + name + " is declared with the calling convention " +
               conventionName(function.getCallingConv()) + " where the header's is C's" + route,
           &function);
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
  // clang -O2 marks the declaration `local_unnamed_addr`; nothing else of these is in what clang writes for it.
  if (function.getUnnamedAddr() == GlobalValue::UnnamedAddr::Global || function.hasSection() ||
      function.hasPartition() || function.hasComdat() || function.hasGC() || function.hasPrefixData() ||
      function.hasPrologueData() || function.hasPersonalityFn() || function.getAlign() ||
      function.getAddressSpace() != 0 || function.isThreadLocal()) {
    misuse("the primitive " + name +
               " is declared with a property the header's declaration does not have (unnamed_addr, a section, a "
               "partition, a comdat, a collector, prefix or prologue data, a personality, an alignment or an "
               "address space)" +
               route,
           &function);
  }
  for (const Use &use : function.uses()) {
    const auto *call = dyn_cast<CallBase>(use.getUser());
    if (call && call->isCallee(&use)) {
      checkCall(*call, primitive, "a call of the device function " + name);
    }
  }
}

// Whether the program uses `function` other than as the target of a direct call: then a call through a pointer
// may reach it.
bool addressTaken(const Function &function) {
  for (const Use &use : function.uses()) {
    const auto *call = dyn_cast<CallBase>(use.getUser());
    if (!call || !call->isCallee(&use)) {
      return true;
    }
  }
  return false;
}

// A primitive call is known by its target, and a call through a pointer reaches a primitive only where the program
// takes its address and the call has its type (any other call through a pointer that reaches it is a call through
// a mismatched prototype, which the device refuses when it runs: `call_indirect` checks the signature). So every
// call through a pointer of a taken primitive's type gets the checks of a direct call; other calls through a
// pointer are the program's own.
void checkIndirectCalls(const Module &module) {
  std::vector<Primitive> taken;
  for (const Primitive &primitive : primitives) {
    if (const Function *function = module.getFunction(primitive.name); function && addressTaken(*function)) {
      taken.push_back(primitive);
    }
  }
  if (taken.empty()) {
    return;
  }
  for (const Function &function : module) {
    for (const BasicBlock &block : function) {
      for (const Instruction &inst : block) {
        const auto *call = dyn_cast<CallBase>(&inst);
        if (!call || call->isInlineAsm() || isa<GlobalValue>(call->getCalledOperand()->stripPointerCasts())) {
          continue;
        }
        for (const Primitive &primitive : taken) {
          if (call->getFunctionType() == primitiveType(primitive, module.getContext())) {
            checkCall(*call, primitive,
                      "a call through a pointer of the type of the device function @" + std::string(primitive.name) +
                          ", which it may reach because the program takes the address of the primitive @" +
                          std::string(primitive.name) + ",");
          }
        }
      }
    }
  }
}

// Whether the program calls or takes the address of `function`.
bool used(const Function *function) { return function && !function->use_empty(); }

// Gives each primitive whose address the program takes a callable device function: an internal function
// `<name>.address` of the primitive's type whose body is the direct call, which every use other than a direct call
// now names. A call through a pointer then reaches the direct call, which the front translates as any other. Only in
// a program from bennett-cc: on the marker path a call of a device function through a pointer stays refused by name
// (front/tests/tests/device.rs), as the old layer has it.
void giveAddresses(Module &module) {
  if (!module.getNamedMetadata(driverMark)) {
    return;
  }
  for (const Primitive &primitive : primitives) {
    Function *function = module.getFunction(primitive.name);
    if (!function || !addressTaken(*function)) {
      continue;
    }
    Function *address = Function::Create(function->getFunctionType(), GlobalValue::InternalLinkage,
                                         std::string(primitive.name) + ".address", module);
    BasicBlock *entry = BasicBlock::Create(module.getContext(), "entry", address);
    IRBuilder<> builder(entry);
    std::vector<Value *> args;
    for (Argument &arg : address->args()) {
      args.push_back(&arg);
    }
    CallInst *call = builder.CreateCall(function, args);
    if (primitive.noreturn) {
      call->setDoesNotReturn();
      address->setDoesNotReturn();
      builder.CreateUnreachable();
    } else if (function->getReturnType()->isVoidTy()) {
      builder.CreateRetVoid();
    } else {
      builder.CreateRet(call);
    }
    function->replaceUsesWithIf(address, [](Use &use) {
      const auto *user = dyn_cast<CallBase>(use.getUser());
      return !user || !user->isCallee(&use);
    });
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
    if (isa<GlobalAlias>(global)) {
      fail("the primitive @" + linker.str() + " is defined by an alias in the program" + route);
    }
    if (isa<GlobalIFunc>(global)) {
      fail("the program defines a function chosen by a resolver (an ifunc) named after the primitive @" +
           linker.str() + route);
    }
    if (isa<GlobalVariable>(global)) {
      misuse("the program has a variable with the linker name of the primitive @" + linker.str() + route, &global);
    }
    checkPrimitiveFunction(cast<Function>(global), *primitive);
  }
  checkIndirectCalls(module);
  const Function *marker = nullptr;
  const Function *handle = nullptr;
  for (const Function &function : module) {
    if (!marker && used(&function) && markerOf(&function)) {
      marker = &function;
    }
    std::optional<Primitive> primitive = primitiveNamed(function.getName());
    if (!handle && used(&function) && primitive && primitive->group == PrimitiveGroup::Interface) {
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
    if (used(&function) && primitive && primitive->group == PrimitiveGroup::Interface) {
      return true;
    }
  }
  return false;
}

void checkHandleProgram(Module &module) {
  checkPrimitives(module);
  for (const Function &function : module) {
    std::optional<Primitive> primitive = primitiveNamed(function.getName());
    if (!used(&function) || !primitive || primitive->group != PrimitiveGroup::Interface) {
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
  giveAddresses(module);
}

} // namespace bennett
