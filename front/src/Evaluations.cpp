#include "Evaluations.h"

#include "Markers.h"
#include "Taint.h"

#include <llvm/IR/Constants.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Metadata.h>
#include <llvm/IR/Module.h>

using namespace llvm;

namespace bennett {

namespace {

const char *const evaluationMetadata = "bennett.evaluation";
const char *const positionPrefix = "position:";

// What `inst` is as an evaluation, or empty when it is copied freely (arithmetic, reads, markers, intrinsics the
// device computes as arithmetic or ignores). Qubit variables are registers, not stack.
std::string kindOf(const Instruction &inst) {
  if (const auto *local = dyn_cast<AllocaInst>(&inst)) {
    return local->getMetadata(qubitMetadata) ? "" : "a reservation of stack";
  }
  const auto *call = dyn_cast<CallBase>(&inst);
  if (!call || markerCall(call)) {
    return "";
  }
  if (const auto *intrinsic = dyn_cast<IntrinsicInst>(call)) {
    switch (intrinsic->getIntrinsicID()) {
    case Intrinsic::returnaddress:
      return std::string(positionPrefix) + "a return address";
    case Intrinsic::frameaddress:
    case Intrinsic::stacksave:
      return std::string(positionPrefix) + "a saved stack pointer";
    case Intrinsic::stackrestore:
      return "a release of stack";
    default:
      return "";
    }
  }
  const Function *callee = call->getCalledFunction();
  return callee ? "a call to @" + callee->getName().str() : "a call through a pointer";
}

std::string kindText(const MDNode *evaluation) {
  return cast<MDString>(evaluation->getOperand(0))->getString().str();
}

} // namespace

void markEvaluations(Module &module, const std::set<const Function *> &control) {
  LLVMContext &context = module.getContext();
  for (Function &function : module) {
    if (!control.count(&function)) {
      continue;
    }
    for (Instruction &inst : instructions(function)) {
      std::string kind = kindOf(inst);
      if (!kind.empty()) {
        inst.setMetadata(evaluationMetadata, MDNode::getDistinct(context, {MDString::get(context, kind)}));
      }
    }
  }
}

const MDNode *evaluationOf(const Instruction &inst) { return inst.getMetadata(evaluationMetadata); }

std::string describeEvaluation(const MDNode *evaluation) {
  std::string text = kindText(evaluation);
  return text.rfind(positionPrefix, 0) == 0 ? text.substr(std::string(positionPrefix).size()) : text;
}

bool readsPosition(const MDNode *evaluation) { return kindText(evaluation).rfind(positionPrefix, 0) == 0; }

Evaluations::Evaluations(const Module &module) {
  for (const Function &function : module) {
    for (const Instruction &inst : instructions(function)) {
      if (const MDNode *evaluation = evaluationOf(inst)) {
        direct[&function].insert(evaluation);
      }
      if (const auto *intrinsic = dyn_cast<IntrinsicInst>(&inst);
          intrinsic && intrinsic->getIntrinsicID() == Intrinsic::returnaddress) {
        const auto *level = dyn_cast<ConstantInt>(intrinsic->getArgOperand(0));
        if (!level || !level->isZero()) {
          callerReaders.insert(&function);
        }
      }
    }
  }
  // What calls a reader reads too; a call through a pointer may reach every function whose address is taken.
  for (bool changed = true; changed;) {
    changed = false;
    for (const Function &function : module) {
      if (function.hasAddressTaken() && callerReaders.count(&function)) {
        anyAddressTakenReader = true;
      }
    }
    for (const Function &function : module) {
      if (callerReaders.count(&function)) {
        continue;
      }
      for (const Instruction &inst : instructions(function)) {
        const auto *call = dyn_cast<CallBase>(&inst);
        if (call && !isa<IntrinsicInst>(call) && readsCallerFrames(*call)) {
          callerReaders.insert(&function);
          changed = true;
          break;
        }
      }
    }
  }
}

const std::set<const MDNode *> &Evaluations::inside(const Function *function) {
  auto found = closed.find(function);
  if (found != closed.end()) {
    return found->second;
  }
  closed[function]; // empty while being computed: a cycle adds nothing more
  std::set<const MDNode *> gathered = direct[function];
  for (const Instruction &inst : instructions(*function)) {
    const auto *call = dyn_cast<CallBase>(&inst);
    const Function *callee = call ? call->getCalledFunction() : nullptr;
    if (callee && !callee->isDeclaration()) {
      const std::set<const MDNode *> &more = inside(callee);
      gathered.insert(more.begin(), more.end());
    }
  }
  closed[function] = gathered;
  return closed[function];
}

bool Evaluations::readsCallerFrames(const CallBase &call) const {
  const Function *callee = call.getCalledFunction();
  return callee ? callerReaders.count(callee) != 0 : anyAddressTakenReader;
}

} // namespace bennett
