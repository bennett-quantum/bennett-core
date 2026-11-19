#include "Unasked.h"

#include "Diagnostic.h"
#include "Markers.h"

#include <llvm/Analysis/AssumptionCache.h>
#include <llvm/Analysis/LoopInfo.h>
#include <llvm/Analysis/ScalarEvolution.h>
#include <llvm/Analysis/TargetLibraryInfo.h>
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

// Intrinsics the device does nothing for, or the bijections the front keeps until after outlining
// (lowerIntrinsics, keepBijections), which are integer operations that cannot fault.
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

// Whether a conditional jump of `loop` tests `divisor` against 0 and every path to `at` takes its nonzero edge.
bool testedNonzero(const Value *divisor, const Instruction &at, const Loop &loop, const DominatorTree &dominators) {
  for (const BasicBlock *block : loop.blocks()) {
    const auto *jump = dyn_cast<BranchInst>(block->getTerminator());
    if (!jump || !jump->isConditional() || jump->getSuccessor(0) == jump->getSuccessor(1)) {
      continue;
    }
    ICmpInst::Predicate predicate;
    if (!match(jump->getCondition(), m_c_ICmp(predicate, m_Specific(divisor), m_Zero())) ||
        (predicate != ICmpInst::ICMP_NE && predicate != ICmpInst::ICMP_EQ)) {
      continue;
    }
    // `divisor != 0` holds on the true edge, `divisor == 0` fails on the false edge.
    const BasicBlock *nonzero = jump->getSuccessor(predicate == ICmpInst::ICMP_NE ? 0 : 1);
    if (dominators.dominates(BasicBlockEdge(block, nonzero), at.getParent())) {
      return true;
    }
  }
  return false;
}

bool divisorProved(const Instruction &division, const Loop *loop, const DominatorTree &dominators) {
  const Value *divisor = division.getOperand(1);
  const APInt *bits = nullptr;
  if ((match(divisor, m_APInt(bits)) || match(divisor, m_c_Or(m_Value(), m_APInt(bits)))) && !bits->isZero()) {
    return true;
  }
  return loop && testedNonzero(divisor, division, *loop, dominators);
}

[[noreturn]] void refuse(const std::string &what, const char *route, const Instruction &inst) {
  notHandled(unaskedContext + what + route, &inst);
}

} // namespace

void requireRunsUnasked(const Instruction &inst, const Loop *loop, const DominatorTree &dominators) {
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
    if (divisorProved(inst, loop, dominators)) {
      return;
    }
    refuse(std::string("`") + inst.getOpcodeName() +
               "` whose divisor is not proved nonzero by a constant or by a test the loop makes, so it can divide "
               "by zero",
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

void warnUncountedGateLoop(const Loop &loop, const Instruction &at) {
  Function &function = *loop.getHeader()->getParent();
  DominatorTree dominators(function);
  LoopInfo loops(dominators);
  const Loop *fresh = loops.getLoopFor(loop.getHeader());
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
  if (isa<SCEVCouldNotCompute>(evolution.getBackedgeTakenCount(fresh))) {
    warning("LLVM's loop analysis finds no number of passes for this loop with gates under the control of a value "
            "that depends on a qubit: the loop's gates are issued whatever the qubits hold, so it runs also where C "
            "does not enter it, and it must end for every ordinary input",
            &at);
  }
}

} // namespace bennett
