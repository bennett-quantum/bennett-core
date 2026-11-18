#include "MovedLoop.h"

#include "Diagnostic.h"

#include <llvm/Analysis/CFG.h>
#include <llvm/Analysis/LoopInfo.h>
#include <llvm/Analysis/ValueTracking.h>
#include <llvm/IR/ConstantRange.h>
#include <llvm/IR/Dominators.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/PatternMatch.h>

#include <set>

using namespace llvm;
using namespace llvm::PatternMatch;

namespace bennett {

namespace {

constexpr const char *tableRoute = " (route: compute the ordinary part inside one applied function for the table of control bits)";

// Use only comparisons that still execute in the moved code. Their ordinary
// values are the device values, so this proof does not use LLVM poison flags.
bool conditionProves(const Value *condition, bool onTrue, CmpInst::Predicate wanted, const Value *left,
                     const Value *right, ConstantRange &possible) {
  if (const auto *test = dyn_cast<ICmpInst>(condition)) {
    auto predicate = onTrue ? test->getPredicate() : test->getInversePredicate();
    const Value *compared = test->getOperand(1);
    if (test->getOperand(1) == left) {
      predicate = CmpInst::getSwappedPredicate(predicate);
      compared = test->getOperand(0);
    } else if (test->getOperand(0) != left) {
      return false;
    }
    if (isa<ConstantInt>(right)) {
      if (const auto *constant = dyn_cast<ConstantInt>(compared)) {
        possible = possible.intersectWith(ConstantRange::makeExactICmpRegion(predicate, constant->getValue()));
      }
    }
    return compared == right && CmpInst::isImpliedTrueByMatchingCmp(predicate, wanted);
  }
  const auto *op = dyn_cast<BinaryOperator>(condition);
  return op && op->getOpcode() == (onTrue ? Instruction::And : Instruction::Or) &&
         (conditionProves(op->getOperand(0), onTrue, wanted, left, right, possible) ||
          conditionProves(op->getOperand(1), onTrue, wanted, left, right, possible));
}

bool entryProves(const Loop &loop, const Loop &root, const DominatorTree &dominators, CmpInst::Predicate predicate,
                 const Value *left, const Value *right) {
  if (const auto *l = dyn_cast<ConstantInt>(left)) {
    if (const auto *r = dyn_cast<ConstantInt>(right)) {
      return ICmpInst::compare(l->getValue(), r->getValue(), predicate);
    }
    std::swap(left, right);
    predicate = CmpInst::getSwappedPredicate(predicate);
  }
  // Combine facts from successive entry tests. A peeled loop starting at 2, for
  // example, may enter only after the bound was ruled out as 0, 1 and 2.
  ConstantRange possible = ConstantRange::getFull(left->getType()->getIntegerBitWidth());
  const BasicBlock *header = loop.getHeader();
  for (auto *node = dominators.getNode(header)->getIDom(); node; node = node->getIDom()) {
    const BasicBlock *block = node->getBlock();
    if (!root.contains(block) && block != root.getLoopPredecessor()) {
      break;
    }
    const auto *jump = dyn_cast<BranchInst>(block->getTerminator());
    if (jump && jump->isConditional() && jump->getSuccessor(0) != jump->getSuccessor(1)) {
      for (bool onTrue : {true, false}) {
        if (dominators.dominates(BasicBlockEdge(block, jump->getSuccessor(onTrue ? 0 : 1)), header) &&
            conditionProves(jump->getCondition(), onTrue, predicate, left, right, possible)) {
          return true;
        }
      }
    }
  }
  if (const auto *constant = dyn_cast<ConstantInt>(right)) {
    return ConstantRange::makeExactICmpRegion(predicate, constant->getValue()).contains(possible);
  }
  return false;
}

bool exitsWithCounter(const Loop &loop, const Loop &root, const DominatorTree &dominators,
                      BasicBlock &exit, BasicBlock &latch, const std::set<const Value *> &available) {
  const auto *jump = dyn_cast<BranchInst>(exit.getTerminator());
  if (!jump || !jump->isConditional()) {
    return false;
  }
  bool staysOnTrue = loop.contains(jump->getSuccessor(0));
  if (staysOnTrue == loop.contains(jump->getSuccessor(1))) {
    return false;
  }
  const auto *test = dyn_cast<ICmpInst>(jump->getCondition());
  if (!test) {
    return false;
  }
  const Value *counter = test->getOperand(0);
  const Value *bound = test->getOperand(1);
  CmpInst::Predicate predicate = staysOnTrue ? test->getPredicate() : test->getInversePredicate();
  if (loop.isLoopInvariant(counter)) {
    std::swap(counter, bound);
    predicate = CmpInst::getSwappedPredicate(predicate);
  }
  if (!counter->getType()->isIntegerTy() || !loop.isLoopInvariant(bound)) {
    return false;
  }
  for (const PHINode &phi : loop.getHeader()->phis()) {
    const Value *next = phi.getIncomingValueForBlock(&latch);
    if (counter != &phi && counter != next) {
      continue;
    }
    bool increments = match(next, m_c_Add(m_Specific(&phi), m_One()));
    bool decrements = !increments && (match(next, m_c_Add(m_Specific(&phi), m_AllOnes())) ||
                                      match(next, m_Sub(m_Specific(&phi), m_One())));
    if (!increments && !decrements) {
      continue;
    }
    const Value *start = phi.getIncomingValueForBlock(loop.getLoopPredecessor());
    auto fixedBeforeControl = [&](const Value *value) {
      return isa<ConstantInt, Argument>(value) || available.count(value);
    };
    if (!fixedBeforeControl(start) || !fixedBeforeControl(bound)) {
      continue;
    }
    if (predicate == ICmpInst::ICMP_NE) {
      // An equality exit must be reachable without first wrapping the counter.
      if (entryProves(loop, root, dominators, ICmpInst::ICMP_ULT,
                      increments ? start : bound, increments ? bound : start)) {
        return true;
      }
      continue;
    }
    bool risesToExit = increments && (predicate == ICmpInst::ICMP_ULT || predicate == ICmpInst::ICMP_SLT);
    bool fallsToExit = decrements && (predicate == ICmpInst::ICMP_UGT || predicate == ICmpInst::ICMP_SGT);
    if (!risesToExit && !fallsToExit) {
      continue;
    }
    // A latch testing the next value must be safe even on the first step,
    // before the comparison has excluded the extreme value.
    if (counter == &phi) {
      return true;
    }
    unsigned width = phi.getType()->getIntegerBitWidth();
    bool isSigned = ICmpInst::isSigned(predicate);
    APInt extreme = increments ? (isSigned ? APInt::getSignedMaxValue(width) : APInt::getMaxValue(width))
                               : (isSigned ? APInt::getSignedMinValue(width) : APInt::getMinValue(width));
    if (entryProves(loop, root, dominators, ICmpInst::ICMP_NE, start, ConstantInt::get(phi.getContext(), extreme))) {
      return true;
    }
  }
  return false;
}

} // namespace

void requireSafeMovedLoopInstruction(const Instruction &inst) {
  if (isa<PHINode, BranchInst, SwitchInst, DbgInfoIntrinsic>(inst)) {
    return;
  }
  if (inst.getOpcode() == Instruction::UDiv || inst.getOpcode() == Instruction::SDiv ||
      inst.getOpcode() == Instruction::URem || inst.getOpcode() == Instruction::SRem) {
    // The device wraps signed overflow. A zero divisor is the only fault. Prove nonzero
    // without facts from the original conditional path or LLVM's overflow and poison rules.
    const Value *divisor = inst.getOperand(1);
    const APInt *bits = nullptr;
    if ((match(divisor, m_APInt(bits)) || match(divisor, m_c_Or(m_Value(), m_APInt(bits)))) && !bits->isZero()) {
      return;
    }
  } else if (!isa<CallBase>(inst) && !inst.mayReadOrWriteMemory() && isSafeToSpeculativelyExecute(&inst)) {
    return;
  }
  notHandled("a moved gate loop would execute ordinary `" + std::string(inst.getOpcodeName()) +
                 "` even where its gates are disabled, without proof that it is safe and has no visible effect" + tableRoute,
             &inst);
}

void requireFiniteMovedLoop(BasicBlock &header, const Instruction &gate,
                            const std::set<const Value *> &available) {
  DominatorTree dominators(*header.getParent());
  LoopInfo loops(dominators);
  const Loop *root = loops.getLoopFor(&header);
  if (!root || root->getHeader() != &header) {
    notHandled("a moved gate loop without a single ordinary loop header" + std::string(tableRoute), &gate);
  }
  using Edge = std::pair<const BasicBlock *, const BasicBlock *>;
  std::set<Edge> counted;
  for (const Loop *loop : root->getLoopsInPreorder()) {
    BasicBlock *latch = loop->getLoopLatch();
    SmallVector<BasicBlock *, 4> exits;
    loop->getExitingBlocks(exits);
    if (!latch || !loop->getLoopPredecessor() || !any_of(exits, [&](BasicBlock *exit) {
          return dominators.dominates(exit, latch) && exitsWithCounter(*loop, *root, dominators, *exit, *latch, available);
        })) {
      notHandled("a moved gate loop whose ordinary passes are not proved to terminate without counter wrap where its gates are disabled with a start and bound fixed before qubit control" +
                     std::string(tableRoute),
                 &gate);
    }
    counted.emplace(latch, loop->getHeader());
  }
  // A cycle that is not one of these loop backedges could bypass every proved counter.
  SmallVector<Edge, 8> backedges;
  FindFunctionBackedges(*gate.getFunction(), backedges);
  for (const auto &edge : backedges) {
    if (root->contains(edge.first) && root->contains(edge.second) && !counted.count(edge)) {
      notHandled("a moved gate loop with an ordinary cycle that can bypass its proved counters" + std::string(tableRoute),
                 &gate);
    }
  }
}

} // namespace bennett
