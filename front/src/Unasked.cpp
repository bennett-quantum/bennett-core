#include "Unasked.h"

#include "Diagnostic.h"
#include "Markers.h"

#include <llvm/Analysis/AssumptionCache.h>
#include <llvm/Analysis/LoopInfo.h>
#include <llvm/Analysis/ScalarEvolution.h>
#include <llvm/Analysis/TargetLibraryInfo.h>
#include <llvm/IR/ConstantRange.h>
#include <llvm/IR/Dominators.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>
#include <llvm/IR/PatternMatch.h>
#include <llvm/TargetParser/Triple.h>

#include <string>

using namespace llvm;
using namespace llvm::PatternMatch;

namespace bennett {

namespace {

const char *const unaskedContext =
    "in a loop with gates under the control of a value that depends on a qubit, whose ordinary code the control "
    "program runs for every input, also where C does not enter the loop: ";
const char *const faultRoute =
    " (route: write the fault code under the control of the loop's condition qubit, not built)";
const char *const effectRoute =
    " (route: compute what the loop needs from it under its condition, in one applied function before the loop "
    "returning a table of control bits, not built)";

// Intrinsics the device does nothing for, or the funnel shifts and byte swaps the front keeps until after
// outlining (lowerIntrinsics, keepBijections): integer operations that cannot fault.
bool harmlessIntrinsic(const IntrinsicInst &intrinsic) {
  if (isa<DbgInfoIntrinsic>(intrinsic) || intrinsic.isLifetimeStartOrEnd()) {
    return true;
  }
  switch (intrinsic.getIntrinsicID()) {
  case Intrinsic::assume:
  case Intrinsic::experimental_noalias_scope_decl:
  case Intrinsic::fshl:
  case Intrinsic::fshr:
  case Intrinsic::bswap:
    return true;
  default:
    return false;
  }
}

// The values `value` may take, computed with the device's arithmetic (which wraps; LLVM's poison flags are not
// used) from constants; anything else may be any value.
ConstantRange rangeOf(const Value *value, unsigned depth) {
  unsigned width = value->getType()->getIntegerBitWidth();
  if (const auto *constant = dyn_cast<ConstantInt>(value)) {
    return ConstantRange(constant->getValue());
  }
  const auto *inst = dyn_cast<Instruction>(value);
  if (!inst || depth == 0) {
    return ConstantRange::getFull(width);
  }
  if (const auto *binary = dyn_cast<BinaryOperator>(inst)) {
    switch (binary->getOpcode()) {
    case Instruction::Add:
    case Instruction::Sub:
    case Instruction::Mul:
    case Instruction::And:
    case Instruction::Or:
    case Instruction::Xor:
    case Instruction::Shl:
    case Instruction::LShr:
    case Instruction::AShr:
      return rangeOf(binary->getOperand(0), depth - 1)
          .binaryOp(binary->getOpcode(), rangeOf(binary->getOperand(1), depth - 1));
    default:
      return ConstantRange::getFull(width);
    }
  }
  if (const auto *cast = dyn_cast<CastInst>(inst);
      cast && (isa<ZExtInst>(cast) || isa<SExtInst>(cast) || isa<TruncInst>(cast))) {
    return rangeOf(cast->getOperand(0), depth - 1).castOp(cast->getOpcode(), width);
  }
  if (const auto *select = dyn_cast<SelectInst>(inst)) {
    return rangeOf(select->getTrueValue(), depth - 1).unionWith(rangeOf(select->getFalseValue(), depth - 1));
  }
  return ConstantRange::getFull(width);
}

// Whether `condition`, where it holds (`onTrue`) or fails, implies `value != 0`: it is `value != 0` (or
// `value == 0` failing), or a conjunction where it holds (a disjunction where it fails) with such a term.
bool impliesNonzero(const Value *condition, bool onTrue, const Value *value) {
  ICmpInst::Predicate predicate;
  if (match(condition, m_c_ICmp(predicate, m_Specific(value), m_Zero()))) {
    return predicate == (onTrue ? ICmpInst::ICMP_NE : ICmpInst::ICMP_EQ);
  }
  const auto *binary = dyn_cast<BinaryOperator>(condition);
  return binary && binary->getOpcode() == (onTrue ? Instruction::And : Instruction::Or) &&
         (impliesNonzero(binary->getOperand(0), onTrue, value) ||
          impliesNonzero(binary->getOperand(1), onTrue, value));
}

// Whether a conditional jump the control program makes implies `divisor != 0` on every path to `at`: the jump
// dominates `at` through its edge on which the test holds.
bool testedNonzero(const Value *divisor, const Instruction &at, const DominatorTree &dominators,
                   const std::set<const BasicBlock *> &untested) {
  const DomTreeNode *node = dominators.getNode(at.getParent());
  for (node = node ? node->getIDom() : nullptr; node; node = node->getIDom()) {
    const BasicBlock *block = node->getBlock();
    const auto *jump = dyn_cast<BranchInst>(block->getTerminator());
    if (untested.count(block) || !jump || !jump->isConditional() || jump->getSuccessor(0) == jump->getSuccessor(1)) {
      continue;
    }
    for (unsigned edge = 0; edge < 2; ++edge) {
      if (impliesNonzero(jump->getCondition(), edge == 0, divisor) &&
          dominators.dominates(BasicBlockEdge(block, jump->getSuccessor(edge)), at.getParent())) {
        return true;
      }
    }
  }
  return false;
}

// Enough for a divisor built from a few operations on constants and masked values.
constexpr unsigned rangeDepth = 6;

bool divisorProved(const Instruction &division, const DominatorTree &dominators,
                   const std::set<const BasicBlock *> &untested) {
  const Value *divisor = division.getOperand(1);
  return !rangeOf(divisor, rangeDepth).contains(APInt::getZero(divisor->getType()->getIntegerBitWidth())) ||
         testedNonzero(divisor, division, dominators, untested);
}

[[noreturn]] void refuse(const std::string &what, const char *route, const Instruction &inst) {
  notHandled(unaskedContext + what + route, &inst);
}

} // namespace

void requireRunsUnasked(const Instruction &inst, const DominatorTree &dominators,
                        const std::set<const BasicBlock *> &untested) {
  switch (inst.getOpcode()) {
  case Instruction::PHI:
  case Instruction::Br:
  case Instruction::Switch:
  case Instruction::ICmp:
  case Instruction::Select:
  case Instruction::GetElementPtr:
  case Instruction::Freeze:
  case Instruction::Add:
  case Instruction::Sub:
  case Instruction::Mul:
  case Instruction::And:
  case Instruction::Or:
  case Instruction::Xor:
  case Instruction::Shl:
  case Instruction::LShr:
  case Instruction::AShr:
  case Instruction::Trunc:
  case Instruction::ZExt:
  case Instruction::SExt:
  case Instruction::PtrToInt:
  case Instruction::IntToPtr:
  case Instruction::BitCast:
    return;
  case Instruction::UDiv:
  case Instruction::SDiv:
  case Instruction::URem:
  case Instruction::SRem:
    if (divisorProved(inst, dominators, untested)) {
      return;
    }
    refuse(std::string("`") + inst.getOpcodeName() +
               "` whose divisor is not proved nonzero, by its range or by a test the control program makes on "
               "every path to it, so it can divide by zero",
           faultRoute, inst);
  case Instruction::Load:
    refuse("a read of memory, which can fault there (out of bounds, or a byte that holds qubits)", faultRoute, inst);
  case Instruction::Alloca:
    refuse("a reservation of stack, which can fault there (stack_overflow)", faultRoute, inst);
  case Instruction::Unreachable:
    refuse("a trap, whose only effect is a fault", faultRoute, inst);
  case Instruction::Store:
  case Instruction::AtomicRMW:
  case Instruction::AtomicCmpXchg:
  case Instruction::Fence:
    refuse("a write of memory, which later code can see", effectRoute, inst);
  case Instruction::Call:
  case Instruction::Invoke:
  case Instruction::CallBr: {
    const auto &call = cast<CallBase>(inst);
    if (const auto *intrinsic = dyn_cast<IntrinsicInst>(&call); intrinsic && harmlessIntrinsic(*intrinsic)) {
      return;
    }
    if (std::optional<Marker> marker = markerCall(&call); marker && marker->kind == MarkerKind::Bit) {
      return;
    }
    const Function *callee = call.getCalledFunction();
    refuse("a call" + (callee ? " to @" + callee->getName().str() : std::string(" through a pointer")) +
               ", which can fault or change what later code sees",
           effectRoute, inst);
  }
  default:
    refuse(std::string("`") + inst.getOpcodeName() + "`, which is not among the instructions known to run there safely",
           faultRoute, inst);
  }
}

void warnUncountedGateLoop(const Loop &loop) {
  Function &function = *loop.getHeader()->getParent();
  DominatorTree dominators(function);
  LoopInfo loops(dominators);
  Loop *fresh = loops.getLoopFor(loop.getHeader());
  while (fresh && fresh->getHeader() != loop.getHeader()) {
    fresh = fresh->getParentLoop();
  }
  if (!fresh) {
    fail("internal: a gate loop that is not a loop of its function");
  }
  TargetLibraryInfoImpl libraryImpl(Triple(function.getParent()->getTargetTriple()));
  TargetLibraryInfo library(libraryImpl, &function);
  AssumptionCache assumptions(function);
  ScalarEvolution evolution(function, library, assumptions, dominators, loops);
  for (Loop *nested : fresh->getLoopsInPreorder()) {
    if (isa<SCEVCouldNotCompute>(evolution.getBackedgeTakenCount(nested))) {
      warning("LLVM's loop analysis finds no number of passes for this loop, in a loop with gates under the control "
              "of a value that depends on a qubit: the loop's gates are issued whatever the qubits hold, so it runs "
              "also where C does not enter it, and it must end for every ordinary input",
              nested->getHeader()->getTerminator());
    }
  }
}

} // namespace bennett
