#include "CallerPosition.h"

#include "Diagnostic.h"

#include <llvm/ADT/SmallPtrSet.h>
#include <llvm/ADT/SmallVector.h>
#include <llvm/Analysis/ValueTracking.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Intrinsics.h>
#include <llvm/IR/Module.h>

using namespace llvm;

namespace bennett {

namespace {

const Function *directTarget(const CallBase &call) {
  return dyn_cast<Function>(call.getCalledOperand()->stripPointerCastsAndAliases());
}

bool exposesStackAddress(const Instruction &inst) {
  if (const auto *address = dyn_cast<PtrToIntInst>(&inst)) {
    SmallVector<const Value *, 4> objects;
    getUnderlyingObjects(address->getPointerOperand(), objects);
    // A pointer loaded from memory or passed to a callee can name a local
    // in an applied frame. Only known static addresses are independent.
    return objects.empty() || !all_of(objects, [](const Value *object) {
             return isa<GlobalValue, ConstantPointerNull>(object);
           });
  }
  const auto *intrinsic = dyn_cast<IntrinsicInst>(&inst);
  if (!intrinsic || intrinsic->getIntrinsicID() != Intrinsic::stacksave) {
    return false;
  }
  // The front lowers frameaddress(0) to stacksave. Saving the stack solely
  // to restore it does not expose the address of the frame.
  return any_of(intrinsic->users(), [](const User *user) {
    const auto *use = dyn_cast<IntrinsicInst>(user);
    return !use || (use->getIntrinsicID() != Intrinsic::stackrestore && !isa<DbgInfoIntrinsic>(use));
  });
}

bool observesThroughDirectCalls(const Function &root, SmallPtrSetImpl<const Function *> &seen,
                                bool &unresolved) {
  SmallVector<const Function *, 8> work{&root};
  while (!work.empty()) {
    const Function *next = work.pop_back_val();
    if (!seen.insert(next).second) {
      continue;
    }
    if (next->getIntrinsicID() == Intrinsic::returnaddress || next->getIntrinsicID() == Intrinsic::frameaddress) {
      return true;
    }
    for (const Instruction &inst : instructions(*next)) {
      if (exposesStackAddress(inst)) {
        return true;
      }
      const auto *call = dyn_cast<CallBase>(&inst);
      if (!call) {
        continue;
      }
      if (const auto *callee = directTarget(*call)) {
        work.push_back(callee);
      } else {
        unresolved = true;
      }
    }
  }
  return false;
}

enum class Observation { None, Direct, Indirect };

Observation observation(const Function *function, const Module &module) {
  SmallPtrSet<const Function *, 32> seen;
  bool unresolved = !function;
  if (function && observesThroughDirectCalls(*function, seen, unresolved)) {
    return Observation::Direct;
  }
  // An unresolved target can name any emitted function, even one whose
  // address came from an ordinary input.
  if (unresolved && any_of(module, [&](const Function &target) {
        return !target.isDeclaration() && observesThroughDirectCalls(target, seen, unresolved);
      })) {
    return Observation::Indirect;
  }
  return Observation::None;
}

void requireCallerIndependent(const Function *function, const Module &module, const Instruction &at,
                              const char *context) {
  Observation observed = observation(function, module);
  if (observed == Observation::None) {
    return;
  }
  std::string message = "replaying a computation that can observe its caller's position through a return or stack address "
                        "(route: keep the old value and release it, or pass the caller's identity as an argument)";
  if (observed == Observation::Indirect) {
    message += " through a possible indirect target";
  }
  if (context) {
    message += context;
  }
  notHandled(message, &at);
}

} // namespace

bool canObserveCallerPosition(const Function &function) {
  return observation(&function, *function.getParent()) != Observation::None;
}

void requireCallerIndependent(const Function &function, const Instruction &at, const char *context) {
  requireCallerIndependent(&function, *function.getParent(), at, context);
}

void requireCallerIndependent(const CallBase &call, const Instruction &at) {
  requireCallerIndependent(directTarget(call), *call.getModule(), at, nullptr);
}

} // namespace bennett
