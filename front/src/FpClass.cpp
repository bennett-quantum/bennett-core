#include "FpClass.h"

#include <llvm/ADT/APFloat.h>
#include <llvm/ADT/FloatingPointMode.h>
#include <llvm/CodeGen/CodeGenCommonISel.h>
#include <llvm/IR/Constants.h>
#include <llvm/IR/IRBuilder.h>

using namespace llvm;

namespace bennett {

Value *classTest(IRBuilderBase &b, Value *bits, Type *type, uint64_t mask, bool strictFP) {
  FPClassTest test = static_cast<FPClassTest>(mask & fcAllFlags);
  if (test == fcNone) {
    return b.getFalse();
  }
  if (test == fcAllFlags) {
    return b.getTrue();
  }
  bool inverted = false;
  if (FPClassTest simpler = invertFPClassTestIfSimpler(test)) {
    inverted = true;
    test = simpler;
  }
  auto finish = [&](Value *result) { return inverted ? b.CreateNot(result) : result; };

  const fltSemantics &semantics = type->getFltSemantics();
  unsigned width = type->getPrimitiveSizeInBits();
  bool x87 = type->isX86_FP80Ty();
  auto constant = [&](const APInt &v) { return ConstantInt::get(bits->getType(), v); };
  APInt zero = APInt::getZero(width);
  APInt sign = APInt::getSignMask(width);
  APInt infinity = APFloat::getInf(semantics).bitcastToAPInt(); // the exponent's bits, and x87's integer bit
  APInt exponentMask = infinity;
  if (x87) {
    exponentMask.clearBit(63);
  }
  APInt fraction = APFloat::getLargest(semantics).bitcastToAPInt() & ~infinity;
  APInt quiet = APInt::getOneBitSet(width, fraction.getActiveBits() - 1);

  Value *magnitude = b.CreateAnd(bits, constant(~sign));
  Value *negative = b.CreateICmpSLT(bits, constant(zero));
  Value *isZero = b.CreateICmpEQ(magnitude, constant(zero));
  Value *exponentIsZero = b.CreateICmpEQ(b.CreateAnd(bits, constant(exponentMask)), constant(zero));
  Value *integerBit = x87 ? b.CreateICmpNE(b.CreateAnd(bits, constant(APInt::getOneBitSet(width, 63))),
                                           constant(zero))
                          : nullptr;
  Value *isNaN = b.CreateICmpUGT(magnitude, constant(infinity));

  if (!strictFP) {
    // The back end compares: a NaN, and for long double an unsupported encoding (an exponent that is not 0
    // with the integer bit clear), is unordered; a pseudo-denormal compares as the number it is.
    Value *unordered =
        x87 ? b.CreateOr(isNaN, b.CreateAnd(b.CreateNot(exponentIsZero), b.CreateNot(integerBit))) : isNaN;
    if (test == fcZero) {
      return finish(isZero);
    }
    if (test == fcNan) {
      return finish(unordered);
    }
    if (test == (fcZero | fcNan)) {
      return finish(b.CreateOr(unordered, isZero));
    }
  }

  // The integer tests, in the back end's order. For float and double the back end tests "finite", "positive
  // finite" and "negative finite" in one comparison each first; those give the same answers as the tests of
  // their classes below, which long double always takes, its integer bit telling the classes apart.
  Value *result = b.getFalse();
  auto add = [&](Value *partial) { result = b.CreateOr(result, partial); };
  if ((test & (fcZero | fcSubnormal)) == (fcZero | fcSubnormal)) {
    add(exponentIsZero);
    test &= ~(fcZero | fcSubnormal);
  }
  if (FPClassTest part = test & fcZero) {
    add(part == fcPosZero ? b.CreateICmpEQ(bits, constant(zero))
        : part == fcZero  ? isZero
                          : b.CreateICmpEQ(bits, constant(sign)));
  }
  if (FPClassTest part = test & fcSubnormal) {
    Value *value = part == fcPosSubnormal ? bits : magnitude;
    Value *partial = b.CreateICmpULT(b.CreateSub(value, constant(APInt(width, 1))), constant(fraction));
    add(part == fcNegSubnormal ? b.CreateAnd(partial, negative) : partial);
  }
  if (FPClassTest part = test & fcInf) {
    add(part == fcPosInf ? b.CreateICmpEQ(bits, constant(infinity))
        : part == fcInf  ? b.CreateICmpEQ(magnitude, constant(infinity))
                         : b.CreateICmpEQ(bits, constant(infinity | sign)));
  }
  if (FPClassTest part = test & fcNan) {
    Value *atLeastQuiet = b.CreateICmpUGE(magnitude, constant(infinity | quiet));
    if (part == fcNan) {
      // "for compatibility with glibc" (LLVM): every encoding whose integer bit equals "the exponent is 0".
      add(x87 ? b.CreateOr(isNaN, b.CreateICmpEQ(integerBit, exponentIsZero)) : isNaN);
    } else if (part == fcQNan) {
      add(atLeastQuiet);
    } else {
      add(b.CreateAnd(isNaN, b.CreateNot(atLeastQuiet)));
    }
  }
  if (FPClassTest part = test & fcNormal) {
    APInt lowest = exponentMask & ~exponentMask.shl(1);
    Value *partial = b.CreateICmpULT(b.CreateSub(magnitude, constant(lowest)), constant(exponentMask - lowest));
    if (part == fcNegNormal) {
      partial = b.CreateAnd(partial, negative);
    } else if (part == fcPosNormal) {
      partial = b.CreateAnd(partial, b.CreateNot(negative));
    }
    add(x87 ? b.CreateAnd(partial, integerBit) : partial);
  }
  return finish(result);
}

} // namespace bennett
