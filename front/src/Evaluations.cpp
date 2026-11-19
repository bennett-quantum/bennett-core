#include "Evaluations.h"

#include "Aliasing.h"
#include "HeldBytes.h"
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

// What a value's computation reads, which must be unchanged where its register is cleared by computing the value
// again (Control.cpp, `recomputable`): the qubit variables it loads, and whether a call or load in it may read
// memory.
struct Inputs {
  std::set<const Storage *> variables;
  bool memory = false;
};

Inputs inputsOf(const Instruction &value, const ControlAnalysis &analysis, const Aliasing &aliasing, bool taintAll) {
  Inputs inputs;
  std::set<const Value *> seen;
  std::vector<const Value *> work{&value};
  while (!work.empty()) {
    const auto *inst = dyn_cast<Instruction>(work.back());
    work.pop_back();
    if (!inst || !seen.insert(inst).second) {
      continue;
    }
    if (const auto *load = dyn_cast<LoadInst>(inst)) {
      if (const Storage *storage = analysis.basedOn(load->getPointerOperand())) {
        inputs.variables.insert(storage);
      } else {
        const auto *global = dyn_cast<GlobalVariable>(getUnderlyingObject(load->getPointerOperand()));
        // An ordinary load is passed in as a value, never read again (with --taint-all too: a load is not moved).
        inputs.memory |= analysis.tainted(load) && (!global || !global->isConstant() || load->isVolatile());
      }
    } else if (const auto *call = dyn_cast<CallBase>(inst); call && !markerCall(call)) {
      // An ordinary call --taint-all moves into the computation runs again with it. Only memory outside the
      // frames of the functions it calls can change before it runs again (Control.cpp, `sharesMemory`).
      // A pointer argument may point into a local of a live caller, which is not private to the call
      // (Aliasing.h counts a local of another function as private).
      const Function *callee = call->getCalledFunction();
      bool pointers = std::any_of(call->arg_begin(), call->arg_end(), [](const Use &arg) {
        return arg->getType()->isPointerTy() || isa<PtrToIntInst>(arg.get());
      });
      inputs.memory |= (taintAll || analysis.tainted(call)) && !(callee && callee->doesNotAccessMemory()) &&
                       (!callee || pointers || aliasing.reachesOutsideFrames(*value.getFunction(), *call));
    }
    for (const Use &operand : inst->operands()) {
      work.push_back(operand.get());
    }
  }
  return inputs;
}

// The reason recorded for an instruction that changes what the value reads before every consumer is done.
std::string unsafe(const std::string &change) {
  return change + " comes between its computation and the place after its consumers (route: keep what it reads "
                  "until the register is cleared, and clear it across a measurement of what it does not read; not "
                  "built)";
}

// A place to clear a register: just before an instruction, or on the edges into `block` from the blocks `from`
// (a block of its own is made there when the place is taken).
struct Clear {
  Instruction *before = nullptr;
  BasicBlock *block = nullptr;
  std::set<BasicBlock *> from;
  bool onEdges() const { return before == nullptr; }
  bool entersBy(const BasicBlock *pred, const BasicBlock *succ) const {
    return onEdges() && succ == block && from.count(const_cast<BasicBlock *>(pred)) != 0;
  }
};

// Why a place is not taken. Unsafe: an instruction on a path to it changes what the value reads, so no later place
// is valid either. Misplaced: the paths do not fit (a later place may).
enum class Outcome { Reached, Unsafe, Misplaced };
struct Verdict {
  Outcome outcome;
  std::string why;
};

// The verdict for a place whose paths do not fit.
Verdict misplaced(const std::string &what) {
  return Verdict{Outcome::Misplaced, what + " (route: clear the register on each path, not built)"};
}

// Follows every path from `first` (included), calling `visit` on each instruction in order and `enter` on each
// edge: an answer Reached ends that path, any other ends the walk with it. A block is entered from its top at most
// once; the block of `first` too, even when `first` is its first instruction (a loop back to `first` is a path that
// reaches it again).
template <typename Visit, typename Enter>
std::optional<Verdict> walkPaths(Instruction &first, Visit visit, Enter enter) {
  std::vector<Instruction *> work{&first};
  std::set<BasicBlock *> entered;
  bool start = true;
  while (!work.empty()) {
    Instruction *inst = work.back();
    work.pop_back();
    bool ended = false;
    for (; inst && !ended; inst = inst->getNextNode()) {
      std::optional<Verdict> answer = visit(*inst, start);
      start = false;
      if (answer && answer->outcome != Outcome::Reached) {
        return answer;
      }
      ended = answer.has_value();
      if (ended || !inst->isTerminator()) {
        continue;
      }
      BasicBlock *block = inst->getParent();
      for (BasicBlock *next : successors(block)) {
        std::optional<Verdict> edge = enter(*block, *next);
        if (edge && edge->outcome != Outcome::Reached) {
          return edge;
        }
        if (!edge && entered.insert(next).second) {
          work.push_back(&next->front());
        }
      }
    }
  }
  return std::nullopt;
}

// The allow-list for a place to clear a register filled just before `fill`: every path to the place ran the fill
// since the place last ran, every path from the fill reaches the place before it returns or fills again, no
// consumer runs after the place before the next fill, and no instruction on a path from the fill to the place
// changes what the value reads: a store, gate or exchange on a qubit variable it reads, a measurement, or, when a
// call in it may read memory, any write to memory (Taint.h, `memoryUnchangedBetween`).
struct ClearCheck {
  Instruction *fill;
  Inputs inputs;
  std::set<const Instruction *> consumers;
  const DominatorTree &dominators;
  const std::vector<BasicBlock *> &qubitJumps;
  std::map<const Instruction *, std::set<const Storage *>> mutated;

  std::optional<std::string> changes(const Instruction &inst) const {
    // The front does not clear a register across a measurement yet, of what the value reads or not.
    std::optional<Marker> marker = markerCall(&inst);
    if (marker && (marker->kind == MarkerKind::Measure || marker->kind == MarkerKind::MeasureBytes)) {
      return "a measurement, across which the front does not clear such a register yet,";
    }
    if (auto found = mutated.find(&inst); found != mutated.end()) {
      for (const Storage *storage : found->second) {
        if (inputs.variables.count(storage)) {
          return "a change of qubit variable " + storage->name + ", which it reads,";
        }
        if (inputs.memory && storage->held) {
          return "a change of memory that holds qubits, which a call in its computation may read,";
        }
      }
    }
    if (!inputs.memory) {
      return std::nullopt;
    }
    if (isExchange(&inst)) {
      return "an exchange of bytes in memory, which a call in its computation may read,";
    }
    const auto *store = dyn_cast<StoreInst>(&inst);
    bool qubitStore = store && mutated.count(store);
    if (inst.mayWriteToMemory() && !qubitStore && !(marker && marker->kind != MarkerKind::MeasureBytes) &&
        !isHolds(&inst)) {
      const auto *call = dyn_cast<CallBase>(&inst);
      const Function *callee = call ? call->getCalledFunction() : nullptr;
      return std::string("a write to memory") + (callee ? " (a call to @" + callee->getName().str() + ")" : "") +
             ", which a call in its computation may read,";
    }
    return std::nullopt;
  }

  std::optional<Verdict> at(const Clear &place) const {
    if (!place.onEdges() && !dominators.dominates(fill, place.before)) {
      return misplaced("a path to the place after its consumers does not compute it");
    }
    // From the fill: the place is reached on every path, and what lies before it keeps what the value reads.
    std::optional<Verdict> forward = walkPaths(
        *fill,
        [&](Instruction &inst, bool start) -> std::optional<Verdict> {
          if (&inst == place.before) {
            return Verdict{Outcome::Reached, ""};
          }
          if (!start && &inst == fill) {
            return misplaced("a path computes it again before the place after its consumers");
          }
          if (std::optional<std::string> change = changes(inst)) {
            return Verdict{Outcome::Unsafe, unsafe(*change)};
          }
          if (isa<ReturnInst, ResumeInst, CleanupReturnInst, CatchReturnInst, CatchSwitchInst>(inst)) {
            return misplaced("a path returns before the place after its consumers");
          }
          return std::nullopt;
        },
        [&](BasicBlock &from, BasicBlock &to) -> std::optional<Verdict> {
          return place.entersBy(&from, &to) ? std::optional<Verdict>(Verdict{Outcome::Reached, ""}) : std::nullopt;
        });
    if (forward) {
      return forward;
    }
    // From the place: the fill comes before the place again and before any consumer.
    Instruction &start = place.onEdges() ? place.block->front() : *place.before;
    return walkPaths(
        start,
        [&](Instruction &inst, bool first) -> std::optional<Verdict> {
          if (&inst == fill) {
            return Verdict{Outcome::Reached, ""};
          }
          if (!first && &inst == place.before) {
            return misplaced("a path reaches the place after its consumers again without computing it");
          }
          if (consumers.count(&inst)) {
            return misplaced("a consumer runs after the place after its consumers on some path");
          }
          return std::nullopt;
        },
        [&](BasicBlock &from, BasicBlock &to) -> std::optional<Verdict> {
          if (place.entersBy(&from, &to)) {
            return misplaced("a path reaches the place after its consumers again without computing it");
          }
          return std::nullopt;
        });
  }

  // Whether a place on edges is inside code under a qubit: a qubit-dependent jump comes before it on every path
  // (it dominates every block the edges leave) and some path from that jump avoids it.
  bool underQubit(const Clear &place) const {
    for (BasicBlock *jumpBlock : qubitJumps) {
      if (!std::all_of(place.from.begin(), place.from.end(),
                       [&](BasicBlock *pred) { return dominators.dominates(jumpBlock, pred); })) {
        continue;
      }
      std::optional<Verdict> avoids = walkPaths(
          *jumpBlock->getTerminator(),
          [&](Instruction &inst, bool) -> std::optional<Verdict> {
            if (isa<ReturnInst, ResumeInst, UnreachableInst, CleanupReturnInst, CatchReturnInst, CatchSwitchInst>(
                    inst)) {
              return Verdict{Outcome::Misplaced, ""};
            }
            return std::nullopt;
          },
          [&](BasicBlock &from, BasicBlock &to) -> std::optional<Verdict> {
            return place.entersBy(&from, &to) ? std::optional<Verdict>(Verdict{Outcome::Reached, ""})
                                              : std::nullopt;
          });
      if (avoids) {
        return true;
      }
    }
    return false;
  }
};

// Registers `call`'s shared value if it has two or more consumers; returns whether it changed the function.
bool shareThroughRegister(Function &function, Instruction &call, std::optional<Aliasing> &aliasing, bool taintAll) {
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
  // Where every consumer is done: the nearest block after all of them on every path, after the last consumer in
  // it, or after the block when that consumer ends it. The register is cleared there or later, at a place that
  // passes the allow-list of `ClearCheck`.
  DominatorTree dominators(function);
  PostDominatorTree after(function);
  LoopInfo loops(dominators);
  BasicBlock *done = fork->getParent();
  for (Instruction *sink : sinks) {
    done = done ? after.findNearestCommonDominator(done, sink->getParent()) : nullptr;
  }
  Instruction *first = nullptr;
  while (done && !first) {
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
    first = last ? last->getNextNode() : &*done->getFirstInsertionPt();
  }
  // Outside code under a qubit: there the clearing is an overwrite in the control program, which computes the old
  // value again by the same generated function (inside, predication would give it a function of its own).
  ControlAnalysis analysis(function, false);
  std::vector<BasicBlock *> qubitJumps;
  for (BasicBlock &jumpBlock : function) {
    const auto *jump = dyn_cast<BranchInst>(jumpBlock.getTerminator());
    const auto *choice = dyn_cast<SwitchInst>(jumpBlock.getTerminator());
    const Value *condition = jump && jump->isConditional() ? jump->getCondition()
                             : choice                      ? choice->getCondition()
                                                           : nullptr;
    if (condition && analysis.tainted(condition)) {
      qubitJumps.push_back(&jumpBlock);
    }
  }
  auto underQubit = [&](BasicBlock *block) {
    return std::any_of(qubitJumps.begin(), qubitJumps.end(), [&](BasicBlock *jumpBlock) {
      return dominators.dominates(jumpBlock, block) && !after.dominates(block, jumpBlock);
    });
  };
  if (underQubit(fork->getParent())) {
    return false; // copied per operation by predication, or part of one applied function already
  }
  // The fill goes just before `fill`.
  Instruction *fill = isa<PHINode>(fork) ? fork->getParent()->getFirstNonPHI() : fork->getNextNode();
  if (!fill) {
    return false; // a call that ends its block (`invoke`): not built
  }
  // The module's aliasing, built at the first value that gets this far (it reads the whole module).
  const Aliasing &moduleAliasing = aliasing ? *aliasing : aliasing.emplace(*function.getParent());
  ClearCheck check{fill, inputsOf(*fork, analysis, moduleAliasing, taintAll), {}, dominators, qubitJumps, {}};
  for (Instruction *node : nodes) {
    if (node != fork && !dominators.dominates(node, fork)) {
      check.consumers.insert(node);
    }
  }
  check.consumers.insert(sinks.begin(), sinks.end());
  for (const auto &storage : analysis.storages) {
    for (Instruction *mutation : storage->mutations) {
      check.mutated[mutation].insert(storage.get());
    }
  }
  // The latest place, from the first one on along blocks every path takes, before the first instruction that
  // changes what the value reads: computing the value again there must give the value the register holds.
  std::optional<Clear> chosen;
  std::string why = "no place after its consumers is reached from its computation on every path (route: clear "
                    "the register on each path, not built)";
  Loop *loop = loops.getLoopFor(fork->getParent());
  auto atBlock = [&](BasicBlock *block) {
    Clear place;
    if (dominators.dominates(fill, &*block->getFirstInsertionPt())) {
      place.before = &*block->getFirstInsertionPt();
      return place;
    }
    place.block = block;
    for (BasicBlock *pred : predecessors(block)) {
      if (dominators.dominates(fill, pred->getTerminator()) &&
          !isa<IndirectBrInst, CallBrInst>(pred->getTerminator())) {
        place.from.insert(pred);
      }
    }
    return place;
  };
  std::optional<Clear> place;
  if (first) {
    place = first == &*first->getParent()->getFirstInsertionPt() ? atBlock(first->getParent())
                                                                  : Clear{first, nullptr, {}};
  }
  while (place) {
    std::optional<Verdict> verdict;
    if (place->onEdges() && (place->from.empty() || place->block->isEHPad())) {
      verdict = misplaced("no path into the place after its consumers that computed it can be cleared on");
    } else if (place->onEdges() ? check.underQubit(*place) : underQubit(place->before->getParent())) {
      verdict = Verdict{Outcome::Misplaced, "every place after its consumers is inside code under a qubit (route: the "
                                            "register inside predicated code, not built)"};
    } else {
      verdict = check.at(*place);
    }
    if (!verdict) {
      chosen = place;
    } else if (verdict->outcome == Outcome::Unsafe) {
      why = verdict->why;
      break;
    } else if (!chosen) {
      why = verdict->why;
    }
    // The next place: within the block, the next instruction (valid where this one was, unless this one changes
    // what the value reads), else the block every path from this one takes next.
    if (!place->onEdges() && !place->before->isTerminator()) {
      Instruction *next = place->before->getNextNode();
      if (chosen && chosen->before == place->before) {
        if (std::optional<std::string> change = check.changes(*place->before)) {
          why = unsafe(*change);
          break;
        }
        place->before = next;
        chosen = place;
        continue; // valid as the place before it was (the paths to it pass that place first)
      }
      place->before = next;
      continue;
    }
    BasicBlock *block = place->onEdges() ? place->block : place->before->getParent();
    DomTreeNode *node = after.getNode(block);
    BasicBlock *next = node && node->getIDom() ? node->getIDom()->getBlock() : nullptr;
    if (!next || (loop && !loop->contains(next))) {
      break; // past the end, or out of the loop: a path would compute the value again before clearing it
    }
    place = atBlock(next);
  }
  if (!chosen) {
    call.setMetadata(sharedRefusedMetadata,
                     MDNode::get(function.getContext(), {MDString::get(function.getContext(), why)}));
    return false;
  }
  // Committed: the clearing place on edges is a block of its own on them.
  Instruction *clearAt = chosen->before;
  if (chosen->onEdges()) {
    std::vector<BasicBlock *> from(chosen->from.begin(), chosen->from.end());
    BasicBlock *edges =
        SplitBlockPredecessors(chosen->block, from, ".cleared", static_cast<DominatorTree *>(nullptr));
    if (!edges) {
      std::string reason = misplaced("its consumers end where LLVM cannot place a block of its own").why;
      call.setMetadata(sharedRefusedMetadata,
                       MDNode::get(function.getContext(), {MDString::get(function.getContext(), reason)}));
      return false;
    }
    clearAt = edges->getTerminator();
  }
  LLVMContext &context = function.getContext();
  unsigned bits = (type->getBitWidth() + 7) / 8 * 8;
  Type *word = IntegerType::get(context, bits);
  IRBuilder<> top(&*function.getEntryBlock().getFirstInsertionPt());
  AllocaInst *variable = top.CreateAlloca(word, nullptr, fork->getName() + ".shared");
  variable->setMetadata(qubitMetadata, MDNode::get(context, {}));
  variable->setMetadata(sharedMetadata, MDNode::get(context, {}));
  top.CreateStore(ConstantInt::get(word, 0), variable); // fresh: holds 0, a value the front knows
  IRBuilder<> b(fill);
  b.SetCurrentDebugLocation(fork->getDebugLoc());
  Value *wide = b.CreateZExt(fork, word);
  Instruction *store = b.CreateStore(wide, variable);
  Value *read = b.CreateTrunc(b.CreateLoad(word, variable, fork->getName() + ".read"), type);
  fork->replaceUsesWithIf(read, [&](Use &use) { return use.getUser() != wide && use.getUser() != store; });
  IRBuilder<> clear(clearAt);
  clear.SetCurrentDebugLocation(fork->getDebugLoc());
  clear.CreateStore(ConstantInt::get(word, 0), variable);
  return true;
}

} // namespace

void registerSharedEvaluations(Function &function, bool taintAll, std::optional<Aliasing> &aliasing) {
  for (bool changed = true; changed;) {
    changed = false;
    ControlAnalysis analysis(function, taintAll);
    for (Instruction &inst : instructions(function)) {
      if (isa<CallBase>(inst) && evaluationOf(inst) && analysis.tainted(&inst) && !inst.getType()->isVoidTy() &&
          !inst.getMetadata(sharedCallMetadata) && !inst.getMetadata(sharedRefusedMetadata) &&
          shareThroughRegister(function, inst, aliasing,
                               taintAll)) {
        inst.setMetadata(sharedCallMetadata, MDNode::get(function.getContext(), {}));
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
