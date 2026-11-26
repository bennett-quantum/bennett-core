#include "Primitives.h"

#include "Diagnostic.h"
#include "Markers.h"

#include <llvm/ADT/ArrayRef.h>
#include <llvm/ADT/SmallVector.h>
#include <llvm/ADT/StringRef.h>
#include <llvm/IR/Attributes.h>
#include <llvm/IR/DerivedTypes.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InstrTypes.h>
#include <llvm/IR/LLVMContext.h>
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

// What an attribute on a primitive's declaration or on a call says, for the declaration rule. The principle
// (docs/DESIGN.md, front end): a program breaks the premise that each primitive call runs and reaches the primitive
// only by telling LLVM a fact about the callee that the header does not state (then LLVM may remove, merge, move or
// speculate the call, or assume what it does to memory or to its result), or by a declaration of another shape. So
// every attribute kind of LLVM 18 is classified once, here, by what it says; the switch has no default, so a kind
// LLVM adds is a compiler warning until it is classified.
enum class Meaning {
  Mode,      // a way of generating code, or an optimisation forbidden: says nothing about what the callee does
  Shape,     // `noreturn`: part of the header's declaration, compared with it
  ValueFact, // a fact about a value passed or returned that clang writes on the header's own declaration
  CallFact,  // a fact about an operand that the optimiser found at a call (it says nothing about the callee)
  Abi,       // how an argument is passed: never on the header's declaration
  Claim,     // a claim about what the callee does or how it may be called: licenses removing or moving a call
};

Meaning meaningOf(Attribute::AttrKind kind) {
  switch (kind) {
  case Attribute::AlwaysInline:
  case Attribute::Cold:
  case Attribute::Convergent:
  case Attribute::CoroDestroyOnlyWhenComplete:
  case Attribute::DisableSanitizerInstrumentation:
  case Attribute::FnRetThunkExtern:
  case Attribute::Hot:
  case Attribute::InlineHint:
  case Attribute::JumpTable:
  case Attribute::MinSize:
  case Attribute::Naked:
  case Attribute::NoBuiltin:
  case Attribute::NoCfCheck:
  case Attribute::NoDuplicate:
  case Attribute::NoImplicitFloat:
  case Attribute::NoInline:
  case Attribute::NoMerge:
  case Attribute::NoProfile:
  case Attribute::NoRedZone:
  case Attribute::NoSanitizeBounds:
  case Attribute::NoSanitizeCoverage:
  case Attribute::NoUnwind: // true of every C function
  case Attribute::NonLazyBind:
  case Attribute::NullPointerIsValid:
  case Attribute::OptForFuzzing:
  case Attribute::OptimizeForDebugging:
  case Attribute::OptimizeForSize:
  case Attribute::OptimizeNone:
  case Attribute::PresplitCoroutine:
  case Attribute::SafeStack:
  case Attribute::SanitizeAddress:
  case Attribute::SanitizeHWAddress:
  case Attribute::SanitizeMemTag:
  case Attribute::SanitizeMemory:
  case Attribute::SanitizeThread:
  case Attribute::ShadowCallStack:
  case Attribute::SkipProfile:
  case Attribute::SpeculativeLoadHardening:
  case Attribute::StackAlignment:
  case Attribute::StackProtect:
  case Attribute::StackProtectReq:
  case Attribute::StackProtectStrong:
  case Attribute::StrictFP:
  case Attribute::UWTable:
  case Attribute::VScaleRange:
    return Meaning::Mode;
  case Attribute::NoReturn:
    return Meaning::Shape;
  case Attribute::NoFPClass: // -ffinite-math-only
  case Attribute::NoUndef:
  case Attribute::SExt:
  case Attribute::ZExt:
    return Meaning::ValueFact;
  case Attribute::Alignment:
  case Attribute::Dereferenceable:
  case Attribute::DereferenceableOrNull:
  case Attribute::NonNull:
    return Meaning::CallFact;
  case Attribute::ByRef:
  case Attribute::ByVal:
  case Attribute::ElementType:
  case Attribute::ImmArg:
  case Attribute::InAlloca:
  case Attribute::InReg:
  case Attribute::Nest:
  case Attribute::Preallocated:
  case Attribute::StructRet:
  case Attribute::SwiftAsync:
  case Attribute::SwiftError:
  case Attribute::SwiftSelf:
    return Meaning::Abi;
  case Attribute::AllocAlign:
  case Attribute::AllocKind:
  case Attribute::AllocSize:
  case Attribute::AllocatedPointer:
  case Attribute::Builtin: // treat the call as a library function's
  case Attribute::DeadOnUnwind:
  case Attribute::Memory:
  case Attribute::MustProgress:
  case Attribute::NoAlias:
  case Attribute::NoCallback:
  case Attribute::NoCapture:
  case Attribute::NoFree:
  case Attribute::NoRecurse:
  case Attribute::NoSync:
  case Attribute::ReadNone:
  case Attribute::ReadOnly:
  case Attribute::Returned:
  case Attribute::ReturnsTwice:
  case Attribute::Speculatable:
  case Attribute::WillReturn:
  case Attribute::Writable:
  case Attribute::WriteOnly:
    return Meaning::Claim;
  case Attribute::None:
  case Attribute::EndAttrKinds:
  case Attribute::EmptyKey:
  case Attribute::TombstoneKey:
    break;
  }
  return Meaning::Claim;
}

// The string attributes clang 18 writes on a declaration or a call for an option (measured over the -O levels, the
// floating-point models and FENV_ACCESS, -fno-builtin, -ffreestanding, PIC and PLT, sanitizers, stack protection,
// -march and -mtune, -fsplit-stack, -mstackrealign, -fzero-call-used-regs, and the profile and coverage options),
// and the code-generation ones clang writes on functions for other options; each asks for a way of generating code.
// LLVM gives meaning to other keys (`alloc-family`, `vector-function-abi-variant`, which replaces a call by
// another), and an unknown key cannot be told from those, so it counts as a claim.
bool codegenString(StringRef key) {
  static constexpr const char *keys[] = {
      "approx-func-fp-math", "branch-target-enforcement", "denormal-fp-math", "denormal-fp-math-f32",
      "frame-pointer", "indirect-tls-seg-refs", "less-precise-fpmad", "min-legal-vector-width", "no-builtins",
      "no-infs-fp-math", "no-jump-tables", "no-nans-fp-math", "no-signed-zeros-fp-math", "no-trapping-math",
      "probe-stack", "sign-return-address", "sign-return-address-key", "split-stack", "stack-probe-size",
      "stack-protector-buffer-size", "stackrealign", "target-cpu", "target-features", "tune-cpu",
      "unsafe-fp-math", "use-soft-float", "warn-stack-size", "zero-call-used-regs",
  };
  for (const char *known : keys) {
    if (key == known) {
      return true;
    }
  }
  return key.starts_with("no-builtin-");
}

// Where an attribute list is: the declaration of a primitive, a direct call of it, or a call through a pointer that
// may reach it. A call through a pointer has a target the front does not know, so only what is independent of the
// target applies to it: no claim about the callee. Its ABI attributes, `noreturn`, type and calling convention
// describe the program's own callback; where they do not fit the primitive the call reaches, the call is through a
// mismatched prototype (undefined in C) and the device stops it where it runs (`bad_indirect` for another type,
// `unreachable` after a `noreturn` call that returns).
enum class Site { Declaration, DirectCall, IndirectCall };

bool accepted(Meaning meaning, Site site) {
  switch (meaning) {
  case Meaning::Mode:
  case Meaning::Shape: // compared with the header's apart from this
  case Meaning::ValueFact:
    return true;
  case Meaning::CallFact:
    return site != Site::Declaration;
  case Meaning::Abi:
    return site == Site::IndirectCall;
  case Meaning::Claim:
    return false;
  }
  return false;
}

// The first attribute of `list` that `site` does not accept, as text, or empty.
std::string firstRefused(const AttributeList &list, unsigned params, Site site) {
  auto refused = [&](const Attribute &attribute) {
    if (attribute.isStringAttribute()) {
      return !codegenString(attribute.getKindAsString());
    }
    return !accepted(meaningOf(attribute.getKindAsEnum()), site);
  };
  for (const Attribute &attribute : list.getFnAttrs()) {
    if (refused(attribute)) {
      return attribute.getAsString();
    }
  }
  for (const Attribute &attribute : list.getRetAttrs()) {
    if (refused(attribute)) {
      return attribute.getAsString() + " on the result";
    }
  }
  for (unsigned i = 0; i < params; ++i) {
    for (const Attribute &attribute : list.getParamAttrs(i)) {
      if (refused(attribute)) {
        return attribute.getAsString() + " on parameter " + std::to_string(i + 1);
      }
    }
  }
  return "";
}

// The first metadata attachment of `value` other than those clang 18 writes for an option and that say nothing about
// what the callee does: debug information, and on a declaration the type identifiers of control-flow integrity
// (`type`, `kcfi_type`), on a call a profile's counts (`prof`). Others (`callback`, `range`, ...) are claims.
template <typename T> std::string firstForeignMetadata(const T &value, bool declaration) {
  SmallVector<std::pair<unsigned, MDNode *>, 4> attached;
  value.getAllMetadata(attached);
  SmallVector<StringRef, 32> names;
  value.getContext().getMDKindNames(names);
  for (const auto &[kind, node] : attached) {
    (void)node;
    StringRef name = kind < names.size() ? names[kind] : StringRef("?");
    bool known = name == "dbg" || (declaration ? (name == "type" || name == "kcfi_type") : name == "prof");
    if (!known) {
      return "!" + name.str();
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

// The checks of a direct call of `primitive`: everything the header's declaration fixes.
void checkDirectCall(const CallBase &call, const Primitive &primitive, const std::string &subject) {
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
  if (call.hasOperandBundles()) {
    misuse(subject + " has an operand bundle, which the header's calls do not have" + route, &call);
  }
}

// The checks every call that may reach `primitive` gets, direct or through a pointer: no attribute or metadata that
// claims something about the callee.
void checkClaims(const CallBase &call, const Primitive &primitive, Site site, const std::string &subject) {
  unsigned params = primitiveType(primitive, call.getContext())->getNumParams();
  if (std::string bad = firstRefused(call.getAttributes(), params, site); !bad.empty()) {
    misuse(subject + " has attributes that let LLVM remove or move its calls (`" + bad + "`)" + route, &call);
  }
  if (std::string bad = firstForeignMetadata(call, false); !bad.empty()) {
    misuse(subject + " has the metadata " + bad + ", which states something about the callee" + route, &call);
  }
}

// The header's declaration as a whole: a symbol with a primitive's linker name must be what clang 18 writes for the
// header's declaration, in each part the LLVM assembly writer prints for a function declaration: linkage,
// preemption, visibility, DLL storage, calling convention, attributes of the result, type, attributes of the
// parameters, unnamed_addr, address space, function attributes, section, partition, comdat, alignment, collector,
// prefix and prologue data, personality and metadata attachments; and no thread-local mode or sanitizer metadata.
// `dso_local` (an option's, -fno-pic) and `local_unnamed_addr` (clang -O2's) say nothing about the callee.
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
  if (std::string bad = firstRefused(function.getAttributes(), expected->getNumParams(), Site::Declaration);
      !bad.empty()) {
    misuse("the primitive " + name + " is declared with `" + bad +
               "`, one of the attributes that let LLVM remove or move its calls" + route,
           &function);
  }
  if (std::string bad = firstForeignMetadata(function, true); !bad.empty()) {
    misuse("the primitive " + name + " is declared with the metadata " + bad +
               ", which states something about the callee" + route,
           &function);
  }
  if (function.getUnnamedAddr() == GlobalValue::UnnamedAddr::Global || function.hasSection() ||
      function.hasPartition() || function.hasComdat() || function.hasGC() || function.hasPrefixData() ||
      function.hasPrologueData() || function.hasPersonalityFn() || function.getAlign() ||
      function.getAddressSpace() != 0 || function.isThreadLocal() || function.hasSanitizerMetadata()) {
    misuse("the primitive " + name +
               " is declared with a property the header's declaration does not have (unnamed_addr, a section, a "
               "partition, a comdat, a collector, prefix or prologue data, a personality, an alignment, an "
               "address space, a thread-local mode or sanitizer metadata)" +
               route,
           &function);
  }
  for (const Use &use : function.uses()) {
    const auto *call = dyn_cast<CallBase>(use.getUser());
    if (call && call->isCallee(&use)) {
      std::string subject = "a call of the device function " + name;
      checkDirectCall(*call, primitive, subject);
      checkClaims(*call, primitive, Site::DirectCall, subject);
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

// A primitive call is known by its target. A call through a pointer reaches a primitive only where the program takes
// its address, and without a mismatched prototype only where the call has the primitive's type; such a call gets the
// checks independent of its target (checkClaims, Site::IndirectCall). Other calls through a pointer are the
// program's own.
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
            checkClaims(*call, primitive, Site::IndirectCall,
                        "a call through a pointer of the type of the device function @" +
                            std::string(primitive.name) + ", which it may reach because the program takes the " +
                            "address of the primitive @" + std::string(primitive.name) + ",");
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
