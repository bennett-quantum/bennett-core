#include "Evaluations.h"

#include "Markers.h"
#include "Taint.h"

#include <llvm/Analysis/ValueTracking.h>

#include <llvm/Analysis/CFG.h>
#include <llvm/Analysis/LoopInfo.h>
#include <llvm/Analysis/PostDominators.h>
#include <llvm/IR/Dominators.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/Transforms/Utils/BasicBlockUtils.h>

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

namespace {

// Instructions a value flows through on its way to a consumer, computed in each consumer's function: arithmetic,
// and calls that return a value (part of the computation, as a floating-point runtime routine is).
bool flowsThrough(const Instruction &inst) {
  if (const auto *call = dyn_cast<CallBase>(&inst)) {
    return !call->getType()->isVoidTy() && !markerCall(call) && !isa<IntrinsicInst>(call);
  }
  return isa<BinaryOperator, CastInst, ICmpInst, SelectInst, FreezeInst>(inst);
}

// The registers' widths: whole bytes, as a qubit variable is as wide as its storage.
constexpr unsigned maxRegisterBits = 64;

// Registers `call`'s shared value if it has two or more consumers; returns whether it changed the function.
bool shareThroughRegister(Function &function, Instruction &call) {
  // The values on the way from the call to its consumers, and the consumers.
  std::vector<Instruction *> nodes{&call};
  std::set<Instruction *> inNodes{&call};
  std::set<Instruction *> sinks;
  for (size_t i = 0; i < nodes.size(); ++i) {
    for (User *user : nodes[i]->users()) {
      auto *inst = cast<Instruction>(user);
      if (!flowsThrough(*inst)) {
        sinks.insert(inst);
      } else if (inNodes.insert(inst).second) {
        nodes.push_back(inst);
      }
    }
  }
  // One consumer is enough when it is a store whose bytes a later store may overwrite: clearing the old value
  // there computes it again, a second evaluation, unless it can read a register.
  bool overwritten = false;
  if (sinks.size() == 1) {
    if (auto *store = dyn_cast<StoreInst>(*sinks.begin())) {
      const Value *object = getUnderlyingObject(store->getPointerOperand());
      for (Instruction &inst : instructions(function)) {
        auto *later = dyn_cast<StoreInst>(&inst);
        overwritten |= later && later != store && getUnderlyingObject(later->getPointerOperand()) == object &&
                       isPotentiallyReachable(store, later);
      }
    }
  }
  if (sinks.size() < 2 && !overwritten) {
    return false;
  }
  // The values every path from the call to each consumer goes through (dominators in this graph of uses).
  std::map<Instruction *, std::set<Instruction *>> through;
  through[&call] = {&call};
  for (bool changed = true; changed;) {
    changed = false;
    for (Instruction *node : nodes) {
      if (node == &call) {
        continue;
      }
      std::optional<std::set<Instruction *>> common;
      for (Use &operand : node->operands()) {
        auto *from = dyn_cast<Instruction>(operand.get());
        if (!from || !inNodes.count(from) || !through.count(from)) {
          continue;
        }
        if (!common) {
          common = through[from];
        } else {
          std::set<Instruction *> both;
          for (Instruction *kept : *common) {
            if (through[from].count(kept)) {
              both.insert(kept);
            }
          }
          common = both;
        }
      }
      if (!common) {
        continue;
      }
      common->insert(node);
      if (through[node] != *common) {
        through[node] = *common;
        changed = true;
      }
    }
  }
  std::optional<std::set<Instruction *>> shared;
  for (Instruction *sink : sinks) {
    for (Use &operand : sink->operands()) {
      auto *from = dyn_cast<Instruction>(operand.get());
      if (!from || !inNodes.count(from)) {
        continue;
      }
      if (!shared) {
        shared = through[from];
      } else {
        std::set<Instruction *> both;
        for (Instruction *kept : *shared) {
          if (through[from].count(kept)) {
            both.insert(kept);
          }
        }
        shared = both;
      }
    }
  }
  Instruction *fork = nullptr;
  for (Instruction *candidate : shared.value_or(std::set<Instruction *>{})) {
    if (!fork || through[candidate].size() > through[fork].size()) {
      fork = candidate;
    }
  }
  if (overwritten) {
    fork = &call; // the call's own value: what follows it may read the variable the store overwrites
  }
  auto *type = fork ? dyn_cast<IntegerType>(fork->getType()) : nullptr;
  if (!type || type->getBitWidth() > maxRegisterBits) {
    return false;
  }
  // Where every consumer is done: the nearest block after all of them on every path, in the fork's loop, reached
  // only through the fork's block; after the last consumer in it, or after the block when that consumer ends it.
  DominatorTree dominators(function);
  PostDominatorTree after(function);
  LoopInfo loops(dominators);
  BasicBlock *done = fork->getParent();
  for (Instruction *sink : sinks) {
    done = done ? after.findNearestCommonDominator(done, sink->getParent()) : nullptr;
  }
  Instruction *clearAt = nullptr;
  while (done && !clearAt) {
    Instruction *last = nullptr;
    for (Instruction *sink : sinks) {
      if (sink->getParent() == done && (!last || last->comesBefore(sink))) {
        last = sink;
      }
    }
    if (last && last->isTerminator()) {
      DomTreeNode *node = after.getNode(done);
      done = node && node->getIDom() ? node->getIDom()->getBlock() : nullptr;
      continue;
    }
    clearAt = last ? last->getNextNode() : &*done->getFirstInsertionPt();
  }
  // Outside code under a qubit: there the clearing is an overwrite in the control program, which computes the old
  // value again by the same generated function (inside, predication would give it a function of its own).
  ControlAnalysis analysis(function, false);
  auto underQubit = [&](BasicBlock *block) {
    for (BasicBlock &jumpBlock : function) {
      const auto *jump = dyn_cast<BranchInst>(jumpBlock.getTerminator());
      const auto *choice = dyn_cast<SwitchInst>(jumpBlock.getTerminator());
      const Value *condition = jump && jump->isConditional() ? jump->getCondition()
                               : choice                      ? choice->getCondition()
                                                             : nullptr;
      if (condition && analysis.tainted(condition) && dominators.dominates(&jumpBlock, block) &&
          !after.dominates(block, &jumpBlock)) {
        return true;
      }
    }
    return false;
  };
  if (underQubit(fork->getParent())) {
    return false; // copied per operation by predication, or part of one applied function already
  }
  while (clearAt && underQubit(clearAt->getParent())) {
    DomTreeNode *node = after.getNode(clearAt->getParent());
    BasicBlock *next = node && node->getIDom() ? node->getIDom()->getBlock() : nullptr;
    clearAt = next ? &*next->getFirstInsertionPt() : nullptr;
  }
  if (!clearAt || loops.getLoopFor(clearAt->getParent()) != loops.getLoopFor(fork->getParent())) {
    return false;
  }
  LLVMContext &context = function.getContext();
  unsigned bits = (type->getBitWidth() + 7) / 8 * 8;
  Type *word = IntegerType::get(context, bits);
  IRBuilder<> top(&*function.getEntryBlock().getFirstInsertionPt());
  AllocaInst *variable = top.CreateAlloca(word, nullptr, fork->getName() + ".shared");
  variable->setMetadata(qubitMetadata, MDNode::get(context, {}));
  variable->setMetadata(sharedMetadata, MDNode::get(context, {}));
  top.CreateStore(ConstantInt::get(word, 0), variable); // fresh: holds 0, a value the front knows
  IRBuilder<> b(isa<PHINode>(fork) ? fork->getParent()->getFirstNonPHI() : fork->getNextNode());
  b.SetCurrentDebugLocation(fork->getDebugLoc());
  Value *wide = b.CreateZExt(fork, word);
  Instruction *store = b.CreateStore(wide, variable);
  Value *read = b.CreateTrunc(b.CreateLoad(word, variable, fork->getName() + ".read"), type);
  fork->replaceUsesWithIf(read, [&](Use &use) { return use.getUser() != wide && use.getUser() != store; });
  // As late as the value can still be computed again from what it read: later code (an overwrite clearing an old
  // value that was computed from this one) may compute it again from the register while the register holds it.
  // Forward from the last consumer, along blocks every path takes, to the first gate, measurement or store on a
  // qubit variable the value reads, or to a return.
  std::set<const Value *> reads;
  {
    std::set<const Value *> seen;
    std::vector<const Value *> work{fork};
    while (!work.empty()) {
      const Value *value = work.back();
      work.pop_back();
      const auto *inst = dyn_cast<Instruction>(value);
      if (!inst || !seen.insert(inst).second) {
        continue;
      }
      if (const auto *load = dyn_cast<LoadInst>(inst)) {
        reads.insert(getUnderlyingObject(load->getPointerOperand()));
      }
      for (const Use &operand : inst->operands()) {
        work.push_back(operand.get());
      }
    }
  }
  auto changesReads = [&](const Instruction &inst) {
    if (isa<ReturnInst>(inst)) {
      return true;
    }
    if (const auto *store = dyn_cast<StoreInst>(&inst)) {
      return reads.count(getUnderlyingObject(store->getPointerOperand())) != 0;
    }
    const auto *call = dyn_cast<CallBase>(&inst);
    if (!call || !markerCall(call)) {
      return false;
    }
    if (std::optional<Marker> marker = markerCall(call);
        marker->kind == MarkerKind::Measure || marker->kind == MarkerKind::MeasureBytes) {
      return true; // before any measurement: the state measured holds no scratch
    }
    for (const Use &arg : call->args()) {
      const Value *pointer = arg.get();
      if (const auto *bit = dyn_cast<CallBase>(pointer); bit && markerCall(bit) && bit->arg_size() > 0) {
        pointer = bit->getArgOperand(0);
      }
      if (pointer->getType()->isPointerTy() && reads.count(getUnderlyingObject(pointer))) {
        return true;
      }
    }
    return false;
  };
  for (;;) {
    bool stopped = false;
    for (Instruction *inst = clearAt; inst; inst = inst->getNextNode()) {
      if (changesReads(*inst) || sinks.count(inst)) {
        clearAt = inst;
        stopped = true;
        break;
      }
      if (inst->isTerminator()) {
        break;
      }
    }
    if (stopped) {
      break;
    }
    DomTreeNode *node = after.getNode(clearAt->getParent());
    BasicBlock *next = node && node->getIDom() ? node->getIDom()->getBlock() : nullptr;
    if (!next || loops.getLoopFor(next) != loops.getLoopFor(fork->getParent()) ||
        !dominators.dominates(fork->getParent(), next)) {
      clearAt = clearAt->getParent()->getTerminator();
      break;
    }
    clearAt = &*next->getFirstInsertionPt();
  }
  IRBuilder<> clear(clearAt);
  clear.SetCurrentDebugLocation(fork->getDebugLoc());
  clear.CreateStore(ConstantInt::get(word, 0), variable);
  return true;
}

} // namespace

void registerSharedEvaluations(Function &function, bool taintAll) {
  for (bool changed = true; changed;) {
    changed = false;
    ControlAnalysis analysis(function, taintAll);
    for (Instruction &inst : instructions(function)) {
      if (isa<CallBase>(inst) && evaluationOf(inst) && analysis.tainted(&inst) && !inst.getType()->isVoidTy() &&
          !inst.getMetadata("bennett.shared") && shareThroughRegister(function, inst)) {
        inst.setMetadata("bennett.shared", MDNode::get(function.getContext(), {}));
        changed = true;
        break;
      }
    }
  }
}

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
