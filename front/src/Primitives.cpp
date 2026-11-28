#include "Primitives.h"

#include "Diagnostic.h"

#include <llvm/ADT/SmallString.h>
#include <llvm/ADT/StringRef.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/GlobalAlias.h>
#include <llvm/IR/GlobalIFunc.h>
#include <llvm/IR/GlobalVariable.h>
#include <llvm/IR/InstrTypes.h>
#include <llvm/IR/Mangler.h>
#include <llvm/IR/Module.h>
#include <llvm/Support/raw_ostream.h>

using namespace llvm;

namespace bennett {

namespace {

// The eleven primitives in the order of docs/HANDLES.md, sections 5.3 to 5.6.
constexpr Primitive primitives[] = {
    {"bq_alloc", PrimitiveGroup::Interface},
    {"bq_free", PrimitiveGroup::Interface},
    {"bq_width", PrimitiveGroup::Interface},
    {"bq_measure", PrimitiveGroup::Interface},
    {"bq_gate", PrimitiveGroup::Interface},
    {"bq_rotate", PrimitiveGroup::Interface},
    {"bq_apply", PrimitiveGroup::Interface},
    {"__bennett_effect", PrimitiveGroup::Effect},
    {"__bennett_exit", PrimitiveGroup::Effect},
    {"__bennett_out_of_memory", PrimitiveGroup::Effect},
    {"__bennett_bad_argument", PrimitiveGroup::Effect},
};

} // namespace

std::string printed(const Type *type) {
  std::string text;
  raw_string_ostream out(text);
  type->print(out);
  return out.str();
}

// The calling convention as the LLVM assembly writer spells the common ones, else by its number (`cc N`).
std::string conventionName(unsigned convention) {
  constexpr std::pair<CallingConv::ID, const char *> named[] = {
      {CallingConv::C, "ccc"},
      {CallingConv::Fast, "fastcc"},
      {CallingConv::Cold, "coldcc"},
      {CallingConv::Win64, "win64cc (ms_abi)"},
      {CallingConv::PreserveMost, "preserve_mostcc"},
  };
  for (const auto &[id, name] : named) {
    if (id == convention) {
      return name;
    }
  }
  return "cc " + std::to_string(convention);
}

namespace {

// The primitive whose linker name in `module` is `linker`, if any.
std::optional<Primitive> primitiveWithLinkerName(StringRef linker, const Module &module) {
  for (const Primitive &primitive : primitives) {
    if (linker == linkerNameOf(primitive.name, module)) {
      return primitive;
    }
  }
  return std::nullopt;
}

} // namespace

const char *const declarationRoute = "; the declarations file bq-prefix.h, which bennett-cc includes by force "
                                     "first in every unit, has the only declaration of each primitive "
                                     "(docs/HANDLES.md, section 4)";

ArrayRef<Primitive> allPrimitives() { return primitives; }

std::optional<Primitive> primitiveNamed(StringRef name) {
  for (const Primitive &primitive : primitives) {
    if (name == primitive.name) {
      return primitive;
    }
  }
  return std::nullopt;
}

std::string linkerName(const GlobalValue &global) {
  SmallString<64> name;
  Mangler().getNameWithPrefix(name, &global, false);
  return name.str().str();
}

std::string linkerNameOf(StringRef name, const Module &module) {
  SmallString<64> mangled;
  Mangler::getNameWithPrefix(mangled, name, module.getDataLayout());
  return mangled.str().str();
}

void checkRedirection(const Module &module) {
  for (const GlobalValue &global : module.global_values()) {
    std::optional<Primitive> primitive = primitiveWithLinkerName(linkerName(global), module);
    if (!primitive) {
      continue;
    }
    std::string name = "@" + std::string(primitive->name);
    if (global.getName().starts_with("\1")) {
      misuse("the program gives a symbol the linker name of the primitive " + name + " with an assembler label" +
                 declarationRoute,
             &global);
    }
    if (isa<GlobalAlias>(global)) {
      fail("the primitive " + name + " is defined by an alias in the program" + declarationRoute);
    }
    if (isa<GlobalIFunc>(global)) {
      fail("the program defines a function chosen by a resolver (an ifunc) named after the primitive " + name +
           declarationRoute);
    }
    if (isa<GlobalVariable>(global)) {
      misuse("the program has a variable with the linker name of the primitive " + name + declarationRoute, &global);
    }
    if (!cast<Function>(global).isDeclaration()) {
      misuse("the program defines the primitive " + name + declarationRoute, &global);
    }
  }
}

void checkCallShape(const Module &module) {
  for (const Primitive &primitive : primitives) {
    const Function *function = module.getFunction(primitive.name);
    if (!function) {
      continue;
    }
    std::string name = "@" + std::string(primitive.name);
    if (!function->hasExternalLinkage()) {
      misuse("the primitive " + name + " is declared weak or with other than external linkage" + declarationRoute,
             function);
    }
    if (function->getCallingConv() != CallingConv::C) {
      misuse("the primitive " + name + " is declared with the calling convention " +
                 conventionName(function->getCallingConv()) + " where the header's is C's" + declarationRoute,
             function);
    }
    for (const Use &use : function->uses()) {
      const auto *call = dyn_cast<CallBase>(use.getUser());
      if (!call || !call->isCallee(&use)) {
        continue;
      }
      if (call->getFunctionType() != function->getFunctionType()) {
        misuse("a call of the device function " + name + " through the type " + printed(call->getFunctionType()) +
                   " where its declaration's is " + printed(function->getFunctionType()) + declarationRoute,
               call);
      }
      if (call->getCallingConv() != function->getCallingConv()) {
        misuse("a call of the device function " + name + " uses the calling convention " +
                   conventionName(call->getCallingConv()) + " where its declaration's is " +
                   conventionName(function->getCallingConv()) + declarationRoute,
               call);
      }
    }
  }
}

bool usesHandleInterface(const Module &module) {
  for (const Primitive &primitive : primitives) {
    const Function *function = module.getFunction(primitive.name);
    if (primitive.group == PrimitiveGroup::Interface && function && !function->use_empty()) {
      return true;
    }
  }
  return false;
}

} // namespace bennett
