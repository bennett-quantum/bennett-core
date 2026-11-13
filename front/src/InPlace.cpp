#include "InPlace.h"

#include "Diagnostic.h"
#include "Taint.h"

#include <llvm/IR/Constants.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/IntrinsicInst.h>

#include <set>

using namespace llvm;

namespace bennett {

namespace {

class Inverter {
public:
  Inverter(ControlAnalysis &analysis, const StoreInst &store, const Storage &target)
      : analysis(analysis), store(store), target(target) {}

  // Whether `value` depends on the variable: reads it, directly or through anything that does.
  bool dependsOnTarget(const Value *value);
  // Whether the low `bits` bits of `value` are a bijection of the variable, as described in InPlace.h.
  bool invertible(const Value *value, unsigned bits);
  // The variable's value, given that the low `bits` bits of `value` are `result`. Only for invertible
  // values.
  Value *inverse(const Value *value, Value *result, unsigned bits, IRBuilder<> &b);

private:
  ControlAnalysis &analysis;
  const StoreInst &store;
  const Storage &target;
  std::map<const Value *, bool> depends;

  // The one operand of a two-operand instruction that depends on the variable, if there is exactly one.
  std::optional<unsigned> dependentOperand(const User *user);
  bool isTargetValue(const Value *value, unsigned bits);
};

bool Inverter::dependsOnTarget(const Value *value) {
  auto found = depends.find(value);
  if (found != depends.end()) {
    return found->second;
  }
  depends[value] = false; // a cycle through phis adds nothing
  bool result = false;
  if (const auto *load = dyn_cast<LoadInst>(value)) {
    std::optional<Access> source = analysis.access(load->getPointerOperand());
    result = source && source->storage == &target;
  }
  if (const auto *user = dyn_cast<Instruction>(value)) {
    for (const Use &use : user->operands()) {
      result = result || dependsOnTarget(use.get());
    }
  }
  depends[value] = result;
  return result;
}

std::optional<unsigned> Inverter::dependentOperand(const User *user) {
  bool first = dependsOnTarget(user->getOperand(0));
  bool second = dependsOnTarget(user->getOperand(1));
  if (first == second) {
    return std::nullopt; // neither (a constant), or both (x + x is not a bijection)
  }
  return first ? 0 : 1;
}

// The variable itself: a whole-width load of it whose value is still the variable's at the store.
bool Inverter::isTargetValue(const Value *value, unsigned bits) {
  const auto *load = dyn_cast<LoadInst>(value);
  std::optional<Access> source = load ? analysis.access(load->getPointerOperand()) : std::nullopt;
  return source && source->storage == &target && source->offset == 0 && bits == target.bits &&
         load->getType()->getIntegerBitWidth() == target.bits && analysis.unchangedBetween(load, &store, &target);
}

uint64_t inverseModPowerOfTwo(uint64_t odd) {
  // Newton's iteration for the inverse modulo 2^64: each step doubles the number of correct low bits, and
  // odd * odd = 1 modulo 8 gives three to start from.
  uint64_t inverse = odd;
  for (unsigned correct = 3; correct < 64; correct *= 2) {
    inverse *= 2 - odd * inverse;
  }
  return inverse;
}

bool Inverter::invertible(const Value *value, unsigned bits) {
  if (isTargetValue(value, bits)) {
    return true;
  }
  const auto *inst = dyn_cast<Instruction>(value);
  if (!inst || !inst->getType()->isIntegerTy() || inst->getType()->getIntegerBitWidth() < bits) {
    return false;
  }
  unsigned width = inst->getType()->getIntegerBitWidth();
  // The low bits of a sum, difference, product or XOR depend only on the low bits of the operands.
  if (const auto *binary = dyn_cast<BinaryOperator>(inst)) {
    std::optional<unsigned> side = dependentOperand(binary);
    if (!side) {
      return false;
    }
    switch (binary->getOpcode()) {
    case Instruction::Add:
    case Instruction::Sub:
    case Instruction::Xor:
      return invertible(binary->getOperand(*side), bits);
    case Instruction::Mul: {
      const auto *factor = dyn_cast<ConstantInt>(binary->getOperand(1 - *side));
      return factor && factor->getValue()[0] && invertible(binary->getOperand(*side), bits);
    }
    default:
      return false;
    }
  }
  if (isa<TruncInst>(inst) || isa<ZExtInst>(inst) || isa<SExtInst>(inst)) {
    const Value *source = inst->getOperand(0);
    return source->getType()->getIntegerBitWidth() >= bits && invertible(source, bits);
  }
  if (const auto *select = dyn_cast<SelectInst>(inst)) {
    return !dependsOnTarget(select->getCondition()) && invertible(select->getTrueValue(), bits) &&
           invertible(select->getFalseValue(), bits);
  }
  if (const auto *intrinsic = dyn_cast<IntrinsicInst>(inst); intrinsic && width == bits) {
    switch (intrinsic->getIntrinsicID()) {
    case Intrinsic::fshl:
    case Intrinsic::fshr:
      // A rotation: both halves are the same value, the amount does not depend on the variable.
      return intrinsic->getArgOperand(0) == intrinsic->getArgOperand(1) &&
             !dependsOnTarget(intrinsic->getArgOperand(2)) && invertible(intrinsic->getArgOperand(0), bits);
    case Intrinsic::bswap:
      return invertible(intrinsic->getArgOperand(0), bits);
    default:
      return false;
    }
  }
  return false;
}

Value *Inverter::inverse(const Value *value, Value *result, unsigned bits, IRBuilder<> &b) {
  if (isTargetValue(value, bits)) {
    return result;
  }
  const auto *inst = cast<Instruction>(value);
  auto low = [&](const Value *operand) { return b.CreateZExtOrTrunc(const_cast<Value *>(operand), b.getIntNTy(bits)); };
  if (const auto *binary = dyn_cast<BinaryOperator>(inst)) {
    unsigned side = *dependentOperand(binary);
    const Value *chain = binary->getOperand(side);
    Value *other = low(binary->getOperand(1 - side));
    switch (binary->getOpcode()) {
    case Instruction::Add: // y = c + o
      return inverse(chain, b.CreateSub(result, other), bits, b);
    case Instruction::Sub: // y = c - o, or y = o - c
      return inverse(chain, side == 0 ? b.CreateAdd(result, other) : b.CreateSub(other, result), bits, b);
    case Instruction::Xor:
      return inverse(chain, b.CreateXor(result, other), bits, b);
    case Instruction::Mul: {
      uint64_t factor = cast<ConstantInt>(binary->getOperand(1 - side))->getZExtValue();
      return inverse(chain, b.CreateMul(result, b.getIntN(bits, inverseModPowerOfTwo(factor))), bits, b);
    }
    default:
      break;
    }
  }
  if (isa<TruncInst>(inst) || isa<ZExtInst>(inst) || isa<SExtInst>(inst)) {
    return inverse(inst->getOperand(0), result, bits, b);
  }
  if (const auto *select = dyn_cast<SelectInst>(inst)) {
    Value *whenTrue = inverse(select->getTrueValue(), result, bits, b);
    Value *whenFalse = inverse(select->getFalseValue(), result, bits, b);
    return b.CreateSelect(const_cast<Value *>(select->getCondition()), whenTrue, whenFalse);
  }
  if (const auto *intrinsic = dyn_cast<IntrinsicInst>(inst)) {
    Value *chain = intrinsic->getArgOperand(0);
    switch (intrinsic->getIntrinsicID()) {
    case Intrinsic::fshl: // a rotation left is undone by the same rotation right
    case Intrinsic::fshr: {
      Intrinsic::ID opposite = intrinsic->getIntrinsicID() == Intrinsic::fshl ? Intrinsic::fshr : Intrinsic::fshl;
      Value *amount = intrinsic->getArgOperand(2);
      return inverse(chain, b.CreateIntrinsic(opposite, {result->getType()}, {result, result, amount}), bits, b);
    }
    case Intrinsic::bswap:
      return inverse(chain, b.CreateUnaryIntrinsic(Intrinsic::bswap, result), bits, b);
    default:
      break;
    }
  }
  fail("internal: inverting a value that is not invertible");
}

// Whether the store is `x ^= e`: the control program applies e into x (ControlWriter::store).
bool isXorStore(const StoreInst &store, const Storage &target, ControlAnalysis &analysis) {
  const auto *x = dyn_cast<BinaryOperator>(store.getValueOperand());
  if (!x || x->getOpcode() != Instruction::Xor) {
    return false;
  }
  for (unsigned side = 0; side < 2; ++side) {
    const auto *load = dyn_cast<LoadInst>(x->getOperand(side));
    if (!load || !analysis.isQuantumLoad(load)) {
      continue;
    }
    Access source = *analysis.access(load->getPointerOperand());
    if (source.storage == &target && source.offset == 0 && load->getType()->getIntegerBitWidth() == target.bits &&
        analysis.unchangedBetween(load, &store, &target)) {
      return true;
    }
  }
  return false;
}

} // namespace

bool rewriteInPlaceUpdate(Function &function, ControlAnalysis &analysis) {
  for (Instruction &inst : instructions(function)) {
    auto *store = dyn_cast<StoreInst>(&inst);
    std::optional<Access> access = store ? analysis.access(store->getPointerOperand()) : std::nullopt;
    if (!access) {
      continue;
    }
    Storage &target = *access->storage;
    Value *value = store->getValueOperand();
    Fact fact = analysis.before(store, &target);
    if (!analysis.tainted(value) || (fact.kind == Fact::Known && analysis.dominators.dominates(fact.value, store)) ||
        isXorStore(*store, target, analysis)) {
      continue;
    }
    Inverter inverter(analysis, *store, target);
    if (!inverter.dependsOnTarget(value) || !inverter.invertible(value, target.bits)) {
      continue; // the control program handles it or refuses it by name
    }

    // A fresh qubit variable of the same width, zero on entry, after each update and at the return.
    LLVMContext &context = function.getContext();
    Type *type = IntegerType::get(context, target.bits);
    IRBuilder<> b(&*function.getEntryBlock().getFirstInsertionPt());
    AllocaInst *next = b.CreateAlloca(type, nullptr, target.alloca->getName() + ".next");
    next->setMetadata(qubitMetadata, MDNode::get(context, {}));

    b.SetInsertPoint(store);
    b.SetCurrentDebugLocation(store->getDebugLoc());
    b.CreateStore(b.CreateXor(b.CreateLoad(type, next), value), next);                     // n ^= g(x, a)
    Value *updated = b.CreateLoad(type, next);
    Value *old = inverter.inverse(value, updated, target.bits, b);                        // g⁻¹(n, a)
    Value *x = b.CreateLoad(type, target.alloca);
    b.CreateStore(b.CreateXor(x, b.CreateXor(old, updated)), target.alloca);              // x ^= g⁻¹(n, a) ^ n
    b.CreateStore(b.CreateXor(b.CreateLoad(type, next), b.CreateLoad(type, target.alloca)), next); // n ^= x
    store->eraseFromParent();
    return true;
  }
  return false;
}

} // namespace bennett
