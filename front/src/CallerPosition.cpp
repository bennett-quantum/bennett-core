#include "CallerPosition.h"

#include "Diagnostic.h"

#include <llvm/ADT/SmallPtrSet.h>
#include <llvm/ADT/SmallVector.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/Intrinsics.h>
#include <llvm/IR/Module.h>

using namespace llvm;

namespace bennett {

bool canReadReturnAddress(const Function &function) {
  SmallPtrSet<const Function *, 32> seen;
  SmallVector<const Function *, 8> work{&function};
  bool consideredIndirectTargets = false;
  while (!work.empty()) {
    const Function *next = work.pop_back_val();
    if (!seen.insert(next).second) {
      continue;
    }
    if (next->getIntrinsicID() == Intrinsic::returnaddress) {
      return true;
    }
    for (const Instruction &inst : instructions(*next)) {
      const auto *call = dyn_cast<CallBase>(&inst);
      if (!call) {
        continue;
      }
      if (const auto *callee = dyn_cast<Function>(call->getCalledOperand()->stripPointerCastsAndAliases())) {
        work.push_back(callee);
      } else if (!consideredIndirectTargets) {
        // An unresolved target can name any emitted function, even one whose
        // address came from an ordinary input.
        for (const Function &target : *function.getParent()) {
          if (!target.isDeclaration()) {
            work.push_back(&target);
          }
        }
        consideredIndirectTargets = true;
      }
    }
  }
  return false;
}

void requireCallerIndependent(const Function &function, const Instruction &at) {
  if (canReadReturnAddress(function)) {
    notHandled("replaying a computation that can observe its caller's position through a return address "
               "(route: keep the old value and release it, or pass the caller's identity as an argument)",
               &at);
  }
}

} // namespace bennett
