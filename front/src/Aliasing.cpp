#include "Aliasing.h"

#include "HeldBytes.h"
#include "Markers.h"

#include <llvm/ADT/SmallVector.h>
#include <llvm/Analysis/ValueTracking.h>
#include <llvm/IR/Constants.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/GlobalAlias.h>
#include <llvm/IR/GlobalVariable.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/Module.h>
#include <llvm/Support/ModRef.h>

#include <algorithm>
#include <optional>
#include <vector>

using namespace llvm;

namespace bennett {

namespace {

// Two addresses that no object has: the sentinels of an object set.
const char escapedTag = 0;
const char anythingTag = 0;

// The calls of `function` in the module, if every use of it is a call of it.
std::optional<std::vector<const CallBase *>> callsOf(const Function &function) {
  std::vector<const CallBase *> calls;
  for (const User *user : function.users()) {
    const auto *call = dyn_cast<CallBase>(user);
    if (!call || call->getCalledOperand() != &function) {
      return std::nullopt;
    }
    calls.push_back(call);
  }
  return calls;
}

// Whether the host, or a call through a pointer, may call `function` with any arguments.
bool openToAnyCaller(const Function &function) { return !function.hasLocalLinkage() || !callsOf(function); }

bool isRestrict(const Argument &argument) { return argument.hasNoAliasAttr(); }

} // namespace

const Value *const Aliasing::escaped = reinterpret_cast<const Value *>(&escapedTag);
const Value *const Aliasing::anything = reinterpret_cast<const Value *>(&anythingTag);

Aliasing::Aliasing(const Module &module) {
  // What each pointer argument may point into: anything for a function open to any caller, otherwise the union
  // of what its calls pass, to a fixed point (the sets only grow).
  for (const Function &function : module) {
    for (const Argument &argument : function.args()) {
      if (argument.getType()->isPointerTy()) {
        arguments[&argument] = openToAnyCaller(function) ? Objects{anything} : Objects{};
      }
    }
  }
  for (bool changed = true; changed;) {
    changed = false;
    for (const Function &function : module) {
      if (openToAnyCaller(function)) {
        continue;
      }
      std::vector<const CallBase *> calls = *callsOf(function);
      for (const CallBase *call : calls) {
        for (const Argument &argument : function.args()) {
          if (!argument.getType()->isPointerTy() || argument.getArgNo() >= call->arg_size()) {
            continue;
          }
          Objects &into = arguments[&argument];
          size_t before = into.size();
          Objects passed = objectsOf(call->getArgOperand(argument.getArgNo()));
          into.insert(passed.begin(), passed.end());
          changed |= into.size() != before;
        }
      }
    }
  }
  // What each defined function may access, through the functions it calls, to a fixed point.
  for (bool changed = true; changed;) {
    changed = false;
    for (const Function &function : module) {
      if (function.isDeclaration()) {
        continue;
      }
      Objects &into = touched[&function];
      size_t before = into.size();
      for (const Instruction &inst : instructions(function)) {
        Objects reached;
        if (const auto *load = dyn_cast<LoadInst>(&inst)) {
          reached = objectsOf(load->getPointerOperand());
        } else if (const auto *store = dyn_cast<StoreInst>(&inst)) {
          reached = objectsOf(store->getPointerOperand());
        } else if (const auto *rmw = dyn_cast<AtomicRMWInst>(&inst)) {
          reached = objectsOf(rmw->getPointerOperand());
        } else if (const auto *exchange = dyn_cast<AtomicCmpXchgInst>(&inst)) {
          reached = objectsOf(exchange->getPointerOperand());
        } else if (const auto *call = dyn_cast<CallBase>(&inst)) {
          reached = touchedBy(*call);
        } else if (inst.mayReadOrWriteMemory() && !isa<FenceInst>(inst)) {
          reached = {anything};
        }
        into.insert(reached.begin(), reached.end());
      }
      changed |= into.size() != before;
    }
  }
}

Aliasing::Objects Aliasing::objectsOf(const Value *pointer) const {
  SmallVector<const Value *, 4> bases;
  getUnderlyingObjects(pointer, bases, nullptr, 0);
  Objects objects;
  for (const Value *base : bases) {
    if (const auto *alias = dyn_cast<GlobalAlias>(base)) {
      base = alias->getAliaseeObject();
    }
    if (const auto *global = dyn_cast_or_null<GlobalVariable>(base)) {
      if (!global->isConstant()) {
        objects.insert(global); // constant memory is never written, so never held
      }
    } else if (!base || isa<ConstantPointerNull>(base) || isa<UndefValue>(base) || isa<Function>(base)) {
      continue;
    } else if (isa<AllocaInst>(base)) {
      objects.insert(base);
    } else if (const auto *argument = dyn_cast<Argument>(base)) {
      if (isRestrict(*argument)) {
        objects.insert(argument); // an object of its own for the call
        continue;
      }
      auto found = arguments.find(argument);
      if (found == arguments.end()) {
        objects.insert(anything);
      } else {
        objects.insert(found->second.begin(), found->second.end());
      }
    } else if (isa<LoadInst>(base) || isa<CallBase>(base)) {
      objects.insert(escaped);
    } else {
      objects.insert(anything); // an address computed from an integer, or what the front does not follow
    }
  }
  return objects;
}

Aliasing::Objects Aliasing::touchedBy(const CallBase &call) const {
  std::optional<Marker> marker = markerCall(&call);
  if (marker && marker->kind == MarkerKind::MeasureBytes) {
    return objectsOf(call.getArgOperand(0)); // the only marker that names memory: it reads the bytes it measures
  }
  if (isExchange(&call)) {
    return objectsOf(call.getArgOperand(1)); // the exchange touches the original bytes
  }
  if (marker || call.doesNotAccessMemory()) {
    return {};
  }
  // The memory this call's pointer arguments point to (its callee's own set says what those arguments may be for
  // any of its calls, which may leave out what only this call passes, such as a local of the caller).
  MemoryEffects effects = call.getMemoryEffects();
  Objects reached;
  if (isModOrRefSet(effects.getModRef(IRMemLocation::ArgMem))) {
    for (const Use &arg : call.args()) {
      if (arg->getType()->isPointerTy()) {
        Objects passed = objectsOf(arg.get());
        reached.insert(passed.begin(), passed.end());
      }
    }
  }
  const Function *callee = call.getCalledFunction();
  if (!callee) {
    reached.insert(anything);
  } else if (!callee->isDeclaration()) {
    if (auto found = touched.find(callee); found != touched.end()) {
      reached.insert(found->second.begin(), found->second.end());
    }
  } else if (isModOrRefSet(effects.getModRef(IRMemLocation::Other))) {
    reached.insert(escaped); // a function the module does not define: any memory whose address escapes
  }
  return reached;
}

// Whether a pointer the front cannot follow may hold an address within `object`: whether that address leaves
// what the front sees, by a store of it, a conversion to an integer, a return, or a call that may keep it.
bool Aliasing::escapes(const Value *object) const {
  auto [found, fresh] = captured.try_emplace(object, false);
  if (!fresh) {
    return found->second;
  }
  std::vector<const Value *> work{object};
  std::set<const Value *> seen{object};
  bool leaves = false;
  while (!work.empty() && !leaves) {
    const Value *value = work.back();
    work.pop_back();
    for (const Use &use : value->uses()) {
      const User *user = use.getUser();
      auto follow = [&] {
        if (seen.insert(user).second) {
          work.push_back(user);
        }
      };
      if (const auto *expression = dyn_cast<ConstantExpr>(user)) {
        if (expression->getOpcode() == Instruction::GetElementPtr || expression->isCast()) {
          if (expression->getType()->isPointerTy()) {
            follow();
            continue;
          }
        }
        leaves = true;
      } else if (isa<LoadInst>(user) || isa<ICmpInst>(user)) {
        continue;
      } else if (const auto *store = dyn_cast<StoreInst>(user)) {
        leaves = store->getValueOperand() == value;
      } else if (const auto *rmw = dyn_cast<AtomicRMWInst>(user)) {
        leaves = rmw->getValOperand() == value;
      } else if (const auto *exchange = dyn_cast<AtomicCmpXchgInst>(user)) {
        leaves = exchange->getPointerOperand() != value;
      } else if (isa<GetElementPtrInst>(user) || isa<BitCastInst>(user) || isa<PHINode>(user) ||
                 isa<SelectInst>(user) || isa<AddrSpaceCastInst>(user)) {
        follow();
      } else if (const auto *call = dyn_cast<CallBase>(user)) {
        bool argument = call->isArgOperand(&use);
        leaves = !(markerCall(call) || isExchange(call) ||
                   (argument && call->doesNotCapture(call->getArgOperandNo(&use))) || call->isBundleOperand(&use));
      } else {
        leaves = true; // a return, an integer conversion, an aggregate holding the address
      }
      if (leaves) {
        break;
      }
    }
  }
  captured[object] = leaves;
  return leaves;
}

bool Aliasing::meet(const Function &function, const Objects &a, const Objects &b) const {
  // An object from `named` may share bytes with an element of `other` that is one of the sentinels.
  auto reaches = [&](const Value *sentinel, const Value *object) {
    if (const auto *argument = dyn_cast<Argument>(object); argument && isRestrict(*argument)) {
      // Only a pointer based on it may reach its bytes: one the front follows, or a copy of it that escaped.
      return escapes(object);
    }
    const auto *local = dyn_cast<AllocaInst>(object);
    if (sentinel == anything) {
      // Nothing outside a call can point into its own frame unless the address escaped.
      return !(local && local->getFunction() == &function) || escapes(object);
    }
    return escapes(object);
  };
  for (const Value *x : a) {
    for (const Value *y : b) {
      bool xSentinel = x == escaped || x == anything;
      bool ySentinel = y == escaped || y == anything;
      if (x == y || (xSentinel && ySentinel && (x == anything || y == anything))) {
        return true;
      }
      if ((xSentinel && !ySentinel && reaches(x, y)) || (ySentinel && !xSentinel && reaches(y, x))) {
        return true;
      }
    }
  }
  return false;
}

void Aliasing::addObjectsOf(const Value *pointer, Objects &into) const {
  Objects objects = objectsOf(pointer);
  into.insert(objects.begin(), objects.end());
}

bool Aliasing::mayReach(const Function &function, const Value *pointer, const Objects &objects) const {
  return !objects.empty() && meet(function, objectsOf(pointer), objects);
}

bool Aliasing::mayShare(const Function &function, const Value *a, const Value *b) const {
  const Value *baseA = getUnderlyingObject(a, 0);
  const Value *baseB = getUnderlyingObject(b, 0);
  // A pointer argument never points into a local variable of its own call (its frame is new).
  auto ownLocal = [&](const Value *x, const Value *y) {
    const auto *local = dyn_cast<AllocaInst>(x);
    return local && local->getFunction() == &function && isa<Argument>(y);
  };
  if (ownLocal(baseA, baseB) || ownLocal(baseB, baseA)) {
    return false;
  }
  return meet(function, objectsOf(a), objectsOf(b));
}

bool Aliasing::mayTouch(const Function &function, const CallBase &call, const Value *pointer) const {
  return meet(function, touchedBy(call), objectsOf(pointer));
}

bool Aliasing::reachesOutsideFrames(const Function &function, const CallBase &call) const {
  Objects reached = touchedBy(call);
  return std::any_of(reached.begin(), reached.end(), [&](const Value *object) {
    const auto *local = dyn_cast<AllocaInst>(object);
    return !local || local->getFunction() == &function;
  });
}

bool mergeEqualArguments(Function &function) {
  if (!function.hasLocalLinkage()) {
    return false;
  }
  std::optional<std::vector<const CallBase *>> calls = callsOf(function);
  if (!calls || calls->empty()) {
    return false;
  }
  bool changed = false;
  for (Argument &first : function.args()) {
    for (Argument &second : function.args()) {
      if (second.getArgNo() <= first.getArgNo() || !first.getType()->isPointerTy() ||
          second.getType() != first.getType() || second.use_empty()) {
        continue;
      }
      bool equal = std::all_of(calls->begin(), calls->end(), [&](const CallBase *call) {
        return call->getArgOperand(first.getArgNo())->stripPointerCasts() ==
               call->getArgOperand(second.getArgNo())->stripPointerCasts();
      });
      if (equal) {
        second.replaceAllUsesWith(&first);
        function.removeParamAttr(first.getArgNo(), Attribute::NoAlias);
        function.removeParamAttr(second.getArgNo(), Attribute::NoAlias);
        changed = true;
      }
    }
  }
  return changed;
}

} // namespace bennett
