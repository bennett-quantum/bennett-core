#include "HandleProgram.h"

#include "Claims.h"
#include "Diagnostic.h"
#include "Markers.h"
#include "Primitives.h"

#include <llvm/IR/Function.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InstrTypes.h>
#include <llvm/IR/Module.h>

#include <optional>
#include <string>
#include <vector>

using namespace llvm;

namespace bennett {

namespace {

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

// The first interface function the module uses, if any.
const Function *firstInterfaceFunction(const Module &module) {
  for (const Primitive &primitive : allPrimitives()) {
    const Function *function = module.getFunction(primitive.name);
    if (primitive.group == PrimitiveGroup::Interface && function && !function->use_empty()) {
      return function;
    }
  }
  return nullptr;
}

const Value *firstUse(const Function &function) {
  for (const User *user : function.users()) {
    if (isa<CallBase>(user)) {
      return user;
    }
  }
  return &function;
}

// Gives each primitive whose address the program takes a callable device function (HandleProgram.h). Only in a
// module from bennett-cc: on the marker path a call of a device function through a pointer stays refused by name
// (front/tests/tests/device.rs), as the old layer has it.
void giveAddresses(Module &module) {
  for (const Primitive &primitive : allPrimitives()) {
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
    if (function->doesNotReturn()) {
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

void checkHandleProgram(Module &module) {
  removePointerCallClaims(module);
  checkRedirection(module);
  checkCallShape(module);
  bool marked = module.getNamedMetadata(driverMark) != nullptr;
  const Function *handle = firstInterfaceFunction(module);
  if (handle && !marked) {
    misuse("the program uses the handle interface (@" + handle->getName().str() +
               ") in bitcode that did not come from bennett-cc; compile each unit with bennett-cc, which checks its "
               "declarations (docs/HANDLES.md, section 4)",
           firstUse(*handle));
  }
  if (handle) {
    for (const Function &function : module) {
      if (!function.use_empty() && markerOf(&function)) {
        misuse("the program uses both the marker interface (@" + function.getName().str() +
                   ", docs/MARKERS.md) and the handle interface (@" + handle->getName().str() +
                   ", docs/HANDLES.md); write it in one of them",
               firstUse(*handle));
      }
    }
    notHandled("the handle interface's @" + handle->getName().str() +
                   ": translating its calls into device instructions waits on the device forms of "
                   "docs/HANDLES.md, section 14",
               firstUse(*handle));
  }
  if (marked) {
    giveAddresses(module);
  }
}

} // namespace bennett
