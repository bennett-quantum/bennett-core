#include "Denormal.h"

#include "Diagnostic.h"

#include <llvm/ADT/FloatingPointMode.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/Instruction.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/Support/raw_ostream.h>

#include <string>

using namespace llvm;

namespace bennett {

namespace {

// Whether the result of `inst` can depend on the subnormal mode. Moves, `fneg`, `fabs`, `copysign` and class
// tests act on the bits; an integer converted to floating point is never subnormal.
bool readsDenormalMode(const Instruction &inst) {
  switch (inst.getOpcode()) {
  case Instruction::FAdd:
  case Instruction::FSub:
  case Instruction::FMul:
  case Instruction::FDiv:
  case Instruction::FRem:
  case Instruction::FCmp:
  case Instruction::FPExt:
  case Instruction::FPTrunc:
  case Instruction::FPToSI:
  case Instruction::FPToUI:
    return true;
  case Instruction::Call: {
    const auto *intrinsic = dyn_cast<IntrinsicInst>(&inst);
    if (!intrinsic || isa<DbgInfoIntrinsic>(intrinsic)) {
      return false;
    }
    switch (intrinsic->getIntrinsicID()) {
    case Intrinsic::fabs:
    case Intrinsic::copysign:
    case Intrinsic::is_fpclass:
      return false;
    default:
      return true;
    }
  }
  default:
    return false;
  }
}

std::string typeName(const Type *type) {
  std::string text;
  raw_string_ostream out(text);
  type->print(out);
  return out.str();
}

void checkType(const Instruction &inst, const Type *type) {
  type = type->getScalarType();
  if (!type->isFloatingPointTy()) {
    return;
  }
  const Function &function = *inst.getFunction();
  DenormalMode mode = function.getDenormalMode(type->getFltSemantics());
  if (mode == DenormalMode::getIEEE()) {
    return;
  }
  const char *attribute =
      type->isFloatTy() && function.getDenormalModeF32Raw().isValid() ? "denormal-fp-math-f32" : "denormal-fp-math";
  notHandled("a floating-point operation on `" + typeName(type) + "` in @" + function.getName().str() +
                 ", which promises the subnormal mode \"" + attribute + "\"=\"" + mode.str() +
                 "\"; the device computes IEEE subnormals (a floating-point environment is a debt)",
             &inst);
}

} // namespace

void checkDenormalMode(const Instruction &inst) {
  if (!readsDenormalMode(inst)) {
    return;
  }
  checkType(inst, inst.getType());
  for (const Use &operand : inst.operands()) {
    checkType(inst, operand->getType());
  }
}

} // namespace bennett
