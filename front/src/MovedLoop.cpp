#include "MovedLoop.h"

#include "Diagnostic.h"

#include <llvm/Analysis/CFG.h>
#include <llvm/Analysis/LoopInfo.h>
#include <llvm/Analysis/ValueTracking.h>
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

// A step of one in either direction visits every value at the device's wrapping integer width.
// A comparison with a loop-invariant bound must eventually leave, even for a start value
// that only reaches this loop with its gates disabled.
bool exitsWithCounter(const Loop &loop, BasicBlock &exit, BasicBlock &latch) {
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
  if (!counter->getType()->isIntegerTy() || !loop.isLoopInvariant(bound) ||
      !(ICmpInst::isEquality(predicate) || ICmpInst::isStrictPredicate(predicate))) {
    return false;
  }
  for (const PHINode &phi : loop.getHeader()->phis()) {
    const Value *next = phi.getIncomingValueForBlock(&latch);
    if (counter != &phi && counter != next) {
      continue;
    }
    if (match(next, m_c_Add(m_Specific(&phi), m_One())) ||
        match(next, m_c_Add(m_Specific(&phi), m_AllOnes())) ||
        match(next, m_Sub(m_Specific(&phi), m_One()))) {
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

void requireFiniteMovedLoop(const Loop &root, const DominatorTree &dominators, const Instruction &gate) {
  using Edge = std::pair<const BasicBlock *, const BasicBlock *>;
  std::set<Edge> counted;
  for (const Loop *loop : root.getLoopsInPreorder()) {
    BasicBlock *latch = loop->getLoopLatch();
    SmallVector<BasicBlock *, 4> exits;
    loop->getExitingBlocks(exits);
    if (!latch || !any_of(exits, [&](BasicBlock *exit) {
          return dominators.dominates(exit, latch) && exitsWithCounter(*loop, *exit, *latch);
        })) {
      notHandled("a moved gate loop whose ordinary passes are not proved to terminate where its gates are disabled" +
                     std::string(tableRoute),
                 &gate);
    }
    counted.emplace(latch, loop->getHeader());
  }
  // A cycle that is not one of these loop backedges could bypass every proved counter.
  SmallVector<Edge, 8> backedges;
  FindFunctionBackedges(*gate.getFunction(), backedges);
  for (const auto &edge : backedges) {
    if (root.contains(edge.first) && root.contains(edge.second) && !counted.count(edge)) {
      notHandled("a moved gate loop with an ordinary cycle that can bypass its proved counters" + std::string(tableRoute),
                 &gate);
    }
  }
}

} // namespace bennett
