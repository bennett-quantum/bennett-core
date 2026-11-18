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

bool containsPointer(Type *type) {
  if (type->isPointerTy()) {
    return true;
  }
  if (auto *array = dyn_cast<ArrayType>(type)) {
    return containsPointer(array->getElementType());
  }
  if (auto *vector = dyn_cast<VectorType>(type)) {
    return containsPointer(vector->getElementType());
  }
  if (auto *structure = dyn_cast<StructType>(type)) {
    return any_of(structure->elements(), containsPointer);
  }
  return false;
}

bool knownStaticAddress(const Value *value) {
  if (!value->getType()->isPointerTy()) {
    return false;
  }
  SmallVector<const Value *, 4> objects;
  getUnderlyingObjects(value, objects);
  return !objects.empty() && all_of(objects, [](const Value *object) {
           return isa<GlobalValue, ConstantPointerNull>(object);
         });
}

bool privateStackSave(const IntrinsicInst &save) {
  return all_of(save.users(), [](const User *user) {
    const auto *use = dyn_cast<IntrinsicInst>(user);
    return use && (use->getIntrinsicID() == Intrinsic::stackrestore || isa<DbgInfoIntrinsic>(use));
  });
}

bool callerIndependentIntrinsic(const IntrinsicInst &call) {
  Intrinsic::ID kind = call.getIntrinsicID();
  if (kind == Intrinsic::stacksave) {
    return privateStackSave(call);
  }
  if (kind == Intrinsic::stackrestore) {
    const auto *save = dyn_cast<IntrinsicInst>(call.getArgOperand(0));
    return save && save->getIntrinsicID() == Intrinsic::stacksave && privateStackSave(*save);
  }
  if (isa<DbgInfoIntrinsic>(call) || call.isLifetimeStartOrEnd()) {
    return true;
  }
  // Simplifying the generated function can reintroduce integer intrinsics.
  // These have only their explicit arguments as inputs.
  static constexpr Intrinsic::ID independent[] = {
      Intrinsic::assume, Intrinsic::experimental_noalias_scope_decl,
      Intrinsic::sadd_with_overflow, Intrinsic::uadd_with_overflow, Intrinsic::ssub_with_overflow,
      Intrinsic::usub_with_overflow, Intrinsic::smul_with_overflow, Intrinsic::umul_with_overflow,
      Intrinsic::sadd_sat, Intrinsic::uadd_sat, Intrinsic::ssub_sat, Intrinsic::usub_sat,
      Intrinsic::bitreverse, Intrinsic::smax, Intrinsic::smin, Intrinsic::umax, Intrinsic::umin,
      Intrinsic::abs, Intrinsic::fshl, Intrinsic::fshr, Intrinsic::bswap, Intrinsic::ctpop,
      Intrinsic::ctlz, Intrinsic::cttz,
  };
  return is_contained(independent, kind);
}

bool keepsPointerPrivate(const Instruction &inst, const Use &use) {
  // An address can select private storage or be carried as a pointer. Treat
  // every other use as observing its representation unless it is static.
  if (isa<GetElementPtrInst, BitCastInst, AddrSpaceCastInst, PHINode, SelectInst, FreezeInst>(inst)) {
    return use->getType()->isPointerTy() && inst.getType()->isPointerTy();
  }
  return (isa<LoadInst>(inst) && use.getOperandNo() == LoadInst::getPointerOperandIndex()) ||
         (isa<StoreInst>(inst) && use.getOperandNo() == StoreInst::getPointerOperandIndex());
}

bool ordinaryDeviceInstruction(const Function &function) {
  if (!function.getReturnType()->isVoidTy() || function.isVarArg() || function.arg_size() != 1) {
    return false;
  }
  Type *operand = function.getArg(0)->getType();
  return (function.getName() == "__bennett_effect" && operand->isIntegerTy(8)) ||
         (function.getName() == "__bennett_exit" && operand->isIntegerTy(32));
}

bool observesThroughDirectCalls(const Function &root, SmallPtrSetImpl<const Function *> &seen,
                                bool &unresolved) {
  SmallVector<const Function *, 8> work{&root};
  while (!work.empty()) {
    const Function *next = work.pop_back_val();
    if (!seen.insert(next).second) {
      continue;
    }
    if (next->isDeclaration()) {
      if (ordinaryDeviceInstruction(*next)) {
        continue; // The writer emits a device instruction with only an integer input.
      }
      return true; // no body to prove independent
    }
    for (const Instruction &inst : instructions(*next)) {
      if (const auto *intrinsic = dyn_cast<IntrinsicInst>(&inst)) {
        if (!callerIndependentIntrinsic(*intrinsic)) {
          return true;
        }
      } else if (const auto *call = dyn_cast<CallBase>(&inst)) {
        if (call->isInlineAsm() || call->hasOperandBundles()) {
          return true;
        }
        // Passing pointers is permitted only if the called body is itself
        // proved caller-independent. Include every possible indirect target.
        if (const auto *callee = directTarget(*call)) {
          if (callee->isDeclaration() && callee->getFunctionType() != call->getFunctionType()) {
            return true;
          }
          work.push_back(callee);
        } else {
          unresolved = true;
        }
      } else {
        for (const Use &operand : inst.operands()) {
          if (containsPointer(operand->getType()) && !knownStaticAddress(operand.get()) &&
              !keepsPointerPrivate(inst, operand)) {
            return true;
          }
        }
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

void requireIndependent(Observation observed, const Instruction &at, const char *context) {
  if (observed == Observation::None) {
    return;
  }
  std::string message = "moving a computation that can observe its caller's position through a return or stack address "
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
  requireIndependent(observation(&function, *function.getParent()), at, context);
}

void requireCallerIndependent(const CallBase &call, const Instruction &at) {
  if (const auto *intrinsic = dyn_cast<IntrinsicInst>(&call)) {
    requireIndependent(callerIndependentIntrinsic(*intrinsic) ? Observation::None : Observation::Direct, at, nullptr);
  } else if (call.isInlineAsm() || call.hasOperandBundles()) {
    requireIndependent(Observation::Direct, at, nullptr);
  } else {
    requireIndependent(observation(directTarget(call), *call.getModule()), at, nullptr);
  }
}

} // namespace bennett
