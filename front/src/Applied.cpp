#include "Applied.h"

#include "Diagnostic.h"

#include <llvm/Analysis/CFG.h>
#include <llvm/Analysis/ValueTracking.h>
#include <llvm/IR/Dominators.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/GlobalVariable.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/IntrinsicInst.h>

#include <map>
#include <set>
#include <string>

using namespace llvm;

namespace bennett {

namespace {

// Adds the object `pointer` is based on to `footprint`, if it is one the footprint names.
void note(Footprint &footprint, const Value *pointer) {
  const Value *object = getUnderlyingObject(pointer);
  if (const auto *global = dyn_cast<GlobalVariable>(object)) {
    footprint.globals.insert(global);
  } else if (const auto *parameter = dyn_cast<Argument>(object)) {
    footprint.parameters.insert(parameter->getArgNo());
  }
}

// `callee`'s footprint seen from a call of it: its globals, and for each of its pointer parameters the
// object the call's argument is based on.
Footprint atCall(const Footprint &callee, const CallBase &call) {
  Footprint seen;
  seen.globals = callee.globals;
  for (unsigned parameter : callee.parameters) {
    if (parameter < call.arg_size()) {
      note(seen, call.getArgOperand(parameter));
    }
  }
  return seen;
}

const Function *directCallee(const Instruction &inst) {
  const auto *call = dyn_cast<CallBase>(&inst);
  const Function *callee = call ? call->getCalledFunction() : nullptr;
  return callee && !callee->isDeclaration() ? callee : nullptr;
}

} // namespace

bool Footprint::add(const Footprint &other) {
  size_t before = globals.size() + parameters.size();
  globals.insert(other.globals.begin(), other.globals.end());
  parameters.insert(other.parameters.begin(), other.parameters.end());
  return globals.size() + parameters.size() != before;
}

// Each function's own accesses first, then those of its callees, to a fixed point.
AppliedCalls::AppliedCalls(const Module &module) {
  for (const Function &function : module) {
    if (function.isDeclaration()) {
      continue;
    }
    Effects &own = effects[&function];
    for (const Instruction &inst : instructions(function)) {
      if (const auto *store = dyn_cast<StoreInst>(&inst)) {
        note(own.writes, store->getPointerOperand());
      } else if (const auto *load = dyn_cast<LoadInst>(&inst)) {
        note(own.reads, load->getPointerOperand());
      } else if (const auto *rmw = dyn_cast<AtomicRMWInst>(&inst)) {
        note(own.writes, rmw->getPointerOperand());
        note(own.reads, rmw->getPointerOperand());
      } else if (const auto *exchange = dyn_cast<AtomicCmpXchgInst>(&inst)) {
        note(own.writes, exchange->getPointerOperand());
        note(own.reads, exchange->getPointerOperand());
      } else if (const auto *intrinsic = dyn_cast<MemIntrinsic>(&inst)) {
        note(own.writes, intrinsic->getDest());
        if (const auto *transfer = dyn_cast<MemTransferInst>(intrinsic)) {
          note(own.reads, transfer->getSource());
        }
      }
    }
  }
  for (bool changed = true; changed;) {
    changed = false;
    for (auto &[function, own] : effects) {
      for (const Instruction &inst : instructions(*function)) {
        const Function *callee = directCallee(inst);
        if (!callee) {
          continue;
        }
        const Effects &theirs = effects.at(callee);
        Footprint writes = atCall(theirs.writes, cast<CallBase>(inst));
        Footprint reads = atCall(theirs.reads, cast<CallBase>(inst));
        changed |= own.writes.add(writes);
        changed |= own.reads.add(reads);
      }
    }
  }
}

namespace {

// An object both footprints name, for a message; empty if none.
std::string shared(const Footprint &read, const Footprint &written) {
  for (const GlobalVariable *global : read.globals) {
    if (written.globals.count(global)) {
      return "@" + global->getName().str();
    }
  }
  for (unsigned parameter : read.parameters) {
    if (written.parameters.count(parameter)) {
      return "the memory its caller passes as argument " + std::to_string(parameter + 1);
    }
  }
  return "";
}

bool onCycle(const BasicBlock &block, const DominatorTree &dominators) {
  for (const BasicBlock *succ : successors(&block)) {
    if (isPotentiallyReachable(succ, &block, nullptr, &dominators)) {
      return true;
    }
  }
  return false;
}

} // namespace

void AppliedCalls::warnUndoneWrites(const CallBase &call, const DominatorTree &dominators) const {
  const Function &caller = *call.getFunction();
  const Function *callee = call.getCalledFunction();
  // The objects the applied call writes, as the calling function names them: its globals and parameters,
  // and its own local variables passed by pointer.
  Footprint written = atCall(effects.at(callee).writes, call);
  std::set<const Value *> locals;
  for (unsigned parameter : effects.at(callee).writes.parameters) {
    if (parameter < call.arg_size()) {
      if (const auto *alloca = dyn_cast<AllocaInst>(getUnderlyingObject(call.getArgOperand(parameter)))) {
        locals.insert(alloca);
      }
    }
  }
  for (const Instruction &inst : instructions(caller)) {
    // Code after the call: what it reaches, and the call itself only when it runs again (on a cycle).
    if (&inst == &call ? !onCycle(*call.getParent(), dominators)
                       : !isPotentiallyReachable(&call, &inst, nullptr, &dominators)) {
      continue;
    }
    Footprint read;
    // Whether code after the call reads one of the caller's local variables the call writes: a load of it,
    // or a direct call that reads the memory it is given a pointer to.
    bool readsLocal = false;
    if (const auto *load = dyn_cast<LoadInst>(&inst)) {
      note(read, load->getPointerOperand());
      readsLocal = locals.count(getUnderlyingObject(load->getPointerOperand())) != 0;
    } else if (const Function *later = directCallee(inst)) {
      const auto &laterCall = cast<CallBase>(inst);
      read = atCall(effects.at(later).reads, laterCall);
      for (unsigned parameter : effects.at(later).reads.parameters) {
        readsLocal = readsLocal || (parameter < laterCall.arg_size() &&
                                    locals.count(getUnderlyingObject(laterCall.getArgOperand(parameter))) != 0);
      }
    }
    if (readsLocal) {
      warning("the call of @" + callee->getName().str() +
                  " depends on a qubit and is applied whole: its writes are undone when it ends, but code after "
                  "it reads a local variable it writes (at " +
                  locationOf(&inst) + ")",
              &call);
      return;
    }
    std::string name = shared(read, written);
    if (!name.empty()) {
      warning("the call of @" + callee->getName().str() +
                  " depends on a qubit and is applied whole: its writes are undone when it ends, but code after it "
                  "reads " +
                  name + ", which it writes (at " + locationOf(&inst) + ")",
              &call);
      return;
    }
  }
}

} // namespace bennett
