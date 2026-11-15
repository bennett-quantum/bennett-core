#include "SoftFloat.h"

#include "Diagnostic.h"
#include "Markers.h"
#include "Normalise.h"

#include <llvm/ADT/StringSet.h>
#include <llvm/Analysis/TargetLibraryInfo.h>
#include <llvm/IR/Constants.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>
#include <llvm/IRReader/IRReader.h>
#include <llvm/Linker/Linker.h>
#include <llvm/Support/SourceMgr.h>
#include <llvm/Support/raw_ostream.h>
#include <llvm/TargetParser/Triple.h>
#include <llvm/Transforms/IPO/Internalize.h>
#include <llvm/Transforms/Utils/Cloning.h>
#include <llvm/Transforms/Utils/Evaluator.h>
#include <llvm/Transforms/Utils/ValueMapper.h>

#include <functional>
#include <map>
#include <memory>
#include <set>
#include <string>
#include <vector>

using namespace llvm;

namespace bennett {

namespace {

// The runtime's definitions are renamed with this prefix: no C name contains a dot.
constexpr const char *runtimePrefix = "bennett.float.";

std::string printedType(const Type *type) {
  std::string text;
  raw_string_ostream out(text);
  type->print(out);
  return out.str();
}

bool hasFloat(const Type *type) {
  if (type->isFloatingPointTy()) {
    return true;
  }
  if (const auto *vector = dyn_cast<VectorType>(type)) {
    return hasFloat(vector->getElementType());
  }
  if (const auto *array = dyn_cast<ArrayType>(type)) {
    return hasFloat(array->getElementType());
  }
  if (const auto *structure = dyn_cast<StructType>(type)) {
    for (const Type *element : structure->elements()) {
      if (hasFloat(element)) {
        return true;
      }
    }
    return false;
  }
  if (const auto *function = dyn_cast<FunctionType>(type)) {
    if (hasFloat(function->getReturnType())) {
      return true;
    }
    for (const Type *param : function->params()) {
      if (hasFloat(param)) {
        return true;
      }
    }
  }
  return false;
}

// A floating-point type the runtime computes on: float, double and x86_fp80 (long double).
bool computable(const Type *type) { return type->isFloatTy() || type->isDoubleTy() || type->isX86_FP80Ty(); }

// The C name of a floating-point type that is not computed on yet.
std::string described(const Type *type) {
  type = type->getScalarType();
  std::string name = "`" + printedType(type) + "`";
  if (type->isX86_FP80Ty()) {
    return name + " (long double)";
  }
  if (type->isHalfTy()) {
    return name + " (_Float16)";
  }
  if (type->isBFloatTy()) {
    return name + " (__bf16)";
  }
  if (type->isFP128Ty()) {
    return name + " (__float128)";
  }
  return name;
}

[[noreturn]] void refuseType(const Type *type, const Value *at) {
  notHandled("an operation on " + described(type) + " values (only float, double and long double are computed on yet)",
             at);
}

// A floating-point operation inside a constant expression.
bool floatOperation(const Constant *constant, std::set<const Constant *> &seen) {
  if (!seen.insert(constant).second || isa<GlobalValue>(constant)) {
    return false;
  }
  if (const auto *expression = dyn_cast<ConstantExpr>(constant)) {
    switch (expression->getOpcode()) {
    case Instruction::FCmp:
    case Instruction::FPToSI:
    case Instruction::FPToUI:
    case Instruction::SIToFP:
    case Instruction::UIToFP:
    case Instruction::FPExt:
    case Instruction::FPTrunc:
    case Instruction::FNeg:
    case Instruction::FAdd:
    case Instruction::FSub:
    case Instruction::FMul:
    case Instruction::FDiv:
    case Instruction::FRem:
      return true;
    default:
      break;
    }
  }
  for (const Use &operand : constant->operands()) {
    if (const auto *inner = dyn_cast<Constant>(operand.get()); inner && floatOperation(inner, seen)) {
      return true;
    }
  }
  return false;
}

// Intrinsics of C library functions: they come with the C library, not the compiler runtime.
bool libraryIntrinsic(Intrinsic::ID id) {
  switch (id) {
  case Intrinsic::sin:
  case Intrinsic::cos:
  case Intrinsic::exp:
  case Intrinsic::exp2:
  case Intrinsic::exp10:
  case Intrinsic::log:
  case Intrinsic::log2:
  case Intrinsic::log10:
  case Intrinsic::pow:
  case Intrinsic::ldexp:
  case Intrinsic::frexp:
    return true;
  default:
    return false;
  }
}

// The bitcode file defining the interface, read when the first routine is needed.
class Runtime {
public:
  Runtime(Module &module, std::string path) : module(module), path(std::move(path)) {}

  // The declaration in `module` of the interface function `name`, of type `type`, after checking that the
  // runtime defines it so. A comparison (`compare`) may return any integer, as libgcc and compiler-rt
  // differ: then the runtime's result type is taken.
  Function *routine(StringRef name, FunctionType *type, bool compare, const Value *at) {
    Module &source = loaded();
    Function *defined = source.getFunction(std::string(runtimePrefix) + name.str());
    if (!defined || defined->isDeclaration()) {
      notHandled("an operation needing @" + name.str() + ", which the floating-point runtime " + path +
                     " does not define",
                 at);
    }
    FunctionType *actual = defined->getFunctionType();
    bool matches = actual == type;
    if (compare && !matches) {
      matches = actual->getReturnType()->isIntegerTy() && actual->params() == type->params() && !actual->isVarArg();
      type = actual;
    }
    if (!matches) {
      notHandled("an operation needing @" + name.str() + ", which the floating-point runtime " + path +
                     " defines with the type " + printedType(actual) + " instead of " + printedType(type),
                 at);
    }
    used = true;
    return cast<Function>(module.getOrInsertFunction(defined->getName(), type).getCallee());
  }

  // Links the routines the program calls, and what they need, and makes them internal.
  bool link() {
    if (!used) {
      return false;
    }
    bool failed = Linker::linkModules(module, std::move(source), Linker::Flags::LinkOnlyNeeded,
                                      [](Module &linked, const StringSet<> &names) {
                                        internalizeModule(linked, [&names](const GlobalValue &value) {
                                          return !value.hasName() || !names.count(value.getName());
                                        });
                                      });
    if (failed) {
      fail("cannot link the floating-point runtime " + path);
    }
    return true;
  }

private:
  Module &loaded() {
    if (source) {
      return *source;
    }
    SMDiagnostic error;
    source = parseIRFile(path, error, module.getContext());
    if (!source) {
      std::string text;
      raw_string_ostream out(text);
      error.print("bennett-front", out);
      fail("cannot read the floating-point runtime " + path + ":\n" + out.str());
    }
    if (source->getDataLayout() != module.getDataLayout()) {
      fail("the floating-point runtime " + path + " was compiled for the data layout `" +
           source->getDataLayoutStr() + "`, the program for `" + module.getDataLayoutStr() + "`");
    }
    source->setTargetTriple(module.getTargetTriple());
    for (GlobalValue &value : source->global_values()) {
      if (!value.isDeclaration() && value.hasName()) {
        value.setName(std::string(runtimePrefix) + value.getName().str());
      }
    }
    return *source;
  }

  Module &module;
  std::string path;
  std::unique_ptr<Module> source;
  bool used = false;
};

// Step 1 for one function. In the runtime itself (`runtime` null) only the inline bit operations are
// allowed: anything else would need the runtime to call itself.
class Lowerer {
public:
  Lowerer(Module &module, Runtime *runtime) : module(module), runtime(runtime) {}

  void lowerFunction(Function &function) {
    std::vector<Instruction *> work;
    for (Instruction &inst : instructions(function)) {
      for (const Use &operand : inst.operands()) {
        std::set<const Constant *> seen;
        if (const auto *constant = dyn_cast<Constant>(operand.get()); constant && floatOperation(constant, seen)) {
          notHandled("a floating-point operation in a constant expression", &inst);
        }
      }
      if (involvesFloat(inst)) {
        work.push_back(&inst);
      }
    }
    for (Instruction *inst : work) {
      lower(inst);
    }
  }

private:
  static bool involvesFloat(const Instruction &inst) {
    if (hasFloat(inst.getType())) {
      return true;
    }
    for (const Use &operand : inst.operands()) {
      if (hasFloat(operand->getType())) {
        return true;
      }
    }
    if (const auto *call = dyn_cast<CallBase>(&inst)) {
      return hasFloat(call->getFunctionType());
    }
    return false;
  }

  // A call of the interface function `name`.
  Value *call(IRBuilder<> &b, StringRef name, Type *result, ArrayRef<Value *> args, const Instruction *at,
              bool compare = false) {
    if (!runtime) {
      notHandled("an operation in the floating-point runtime that needs @" + name.str() +
                     " (the runtime must be integer code)",
                 at);
    }
    std::vector<Type *> params;
    for (Value *arg : args) {
      params.push_back(arg->getType());
    }
    Function *routine = runtime->routine(name, FunctionType::get(result, params, false), compare, at);
    CallInst *made = b.CreateCall(routine, args);
    return made;
  }

  // The suffix of a libgcc name for `type`: sf, df or xf.
  static std::string mode(const Type *type) {
    return type->isFloatTy() ? "sf" : type->isDoubleTy() ? "df" : "xf";
  }
  // The suffix of a C library name for `type`: f, nothing or l.
  static std::string libm(const Type *type) { return type->isFloatTy() ? "f" : type->isDoubleTy() ? "" : "l"; }

  static Type *bitsOf(Type *type) { return IntegerType::get(type->getContext(), type->getPrimitiveSizeInBits()); }
  static Value *bits(IRBuilder<> &b, Value *x) { return b.CreateBitCast(x, bitsOf(x->getType())); }
  static APInt signBit(Type *type) { return APInt::getSignMask(type->getPrimitiveSizeInBits()); }

  // Lowers what `b` made, if it is an instruction, and returns what replaces it.
  Value *lowerMade(Value *value) {
    auto *inst = dyn_cast<Instruction>(value);
    return inst ? lower(inst) : value;
  }

  void replace(Instruction *inst, Value *with) {
    with->takeName(inst);
    inst->replaceAllUsesWith(with);
    inst->eraseFromParent();
  }

  // Whether `inst` is an operation done element by element on vectors of floating-point values.
  static bool vectorOperation(const Instruction &inst) {
    switch (inst.getOpcode()) {
    case Instruction::FNeg:
    case Instruction::FAdd:
    case Instruction::FSub:
    case Instruction::FMul:
    case Instruction::FDiv:
    case Instruction::FCmp:
    case Instruction::FPExt:
    case Instruction::FPTrunc:
    case Instruction::FPToSI:
    case Instruction::FPToUI:
    case Instruction::SIToFP:
    case Instruction::UIToFP:
      break;
    case Instruction::Call:
      if (!isa<IntrinsicInst>(inst) || isa<DbgInfoIntrinsic>(inst)) {
        return false;
      }
      break;
    default:
      return false;
    }
    if (inst.getType()->isVectorTy()) {
      return true;
    }
    for (const Use &operand : inst.operands()) {
      if (operand->getType()->isVectorTy()) {
        return true;
      }
    }
    return false;
  }

  Value *scalarise(Instruction *inst) {
    auto *vector = dyn_cast<FixedVectorType>(inst->getType()->isVectorTy() ? inst->getType()
                                                                           : inst->getOperand(0)->getType());
    if (!vector) {
      notHandled("a floating-point operation on vectors of scalable length", inst);
    }
    Function *scalarCallee = nullptr;
    if (auto *intrinsic = dyn_cast<IntrinsicInst>(inst)) {
      SmallVector<Type *, 4> overloads;
      if (!Intrinsic::getIntrinsicSignature(intrinsic->getCalledFunction(), overloads)) {
        fail("internal: the signature of @" + intrinsic->getCalledFunction()->getName().str());
      }
      for (Type *&type : overloads) {
        type = type->getScalarType();
      }
      scalarCallee = Intrinsic::getDeclaration(&module, intrinsic->getIntrinsicID(), overloads);
    }
    IRBuilder<> b(inst);
    Value *result = PoisonValue::get(inst->getType());
    for (unsigned lane = 0; lane < vector->getNumElements(); ++lane) {
      Instruction *scalar = inst->clone();
      for (Use &operand : scalar->operands()) {
        if (operand->getType()->isVectorTy()) {
          operand.set(b.CreateExtractElement(operand.get(), lane));
        }
      }
      scalar->mutateType(inst->getType()->getScalarType());
      if (scalarCallee) {
        auto *call = cast<CallInst>(scalar);
        call->mutateFunctionType(scalarCallee->getFunctionType());
        call->setCalledFunction(scalarCallee);
      }
      b.Insert(scalar);
      result = b.CreateInsertElement(result, lower(scalar), lane);
    }
    replace(inst, result);
    return result;
  }

  Value *compare(IRBuilder<> &b, FCmpInst::Predicate predicate, Value *x, Value *y, const Instruction *at) {
    Type *type = x->getType();
    if (predicate == FCmpInst::FCMP_FALSE) {
      return b.getFalse();
    }
    if (predicate == FCmpInst::FCMP_TRUE) {
      return b.getTrue();
    }
    // TargetLowering::softenSetCCOperands: one or two calls, each compared with 0, and an inverted
    // predicate as the inverse integer comparison (two calls are then joined with `and`, not `or`).
    struct Test {
      const char *name;
      CmpInst::Predicate test;
    };
    Test first{nullptr, CmpInst::ICMP_EQ}, second{nullptr, CmpInst::ICMP_EQ};
    bool invert = false;
    switch (predicate) {
    case FCmpInst::FCMP_OEQ:
      first = {"eq", CmpInst::ICMP_EQ};
      break;
    case FCmpInst::FCMP_UNE:
      first = {"ne", CmpInst::ICMP_NE};
      break;
    case FCmpInst::FCMP_OGE:
      first = {"ge", CmpInst::ICMP_SGE};
      break;
    case FCmpInst::FCMP_OLT:
      first = {"lt", CmpInst::ICMP_SLT};
      break;
    case FCmpInst::FCMP_OLE:
      first = {"le", CmpInst::ICMP_SLE};
      break;
    case FCmpInst::FCMP_OGT:
      first = {"gt", CmpInst::ICMP_SGT};
      break;
    case FCmpInst::FCMP_ORD:
      invert = true;
      [[fallthrough]];
    case FCmpInst::FCMP_UNO:
      first = {"unord", CmpInst::ICMP_NE};
      break;
    case FCmpInst::FCMP_ONE:
      invert = true;
      [[fallthrough]];
    case FCmpInst::FCMP_UEQ:
      first = {"unord", CmpInst::ICMP_NE};
      second = {"eq", CmpInst::ICMP_EQ};
      break;
    case FCmpInst::FCMP_ULT:
      invert = true;
      first = {"ge", CmpInst::ICMP_SGE};
      break;
    case FCmpInst::FCMP_ULE:
      invert = true;
      first = {"gt", CmpInst::ICMP_SGT};
      break;
    case FCmpInst::FCMP_UGT:
      invert = true;
      first = {"le", CmpInst::ICMP_SLE};
      break;
    case FCmpInst::FCMP_UGE:
      invert = true;
      first = {"lt", CmpInst::ICMP_SLT};
      break;
    default:
      fail("internal: the fcmp predicate " + std::to_string(predicate));
    }
    auto test = [&](const Test &t) {
      CmpInst::Predicate p = invert ? CmpInst::getInversePredicate(t.test) : t.test;
      Value *result = call(b, std::string("__") + t.name + mode(type) + "2", b.getInt32Ty(), {x, y}, at, true);
      return b.CreateICmp(p, result, ConstantInt::get(result->getType(), 0));
    };
    Value *result = test(first);
    if (second.name) {
      result = invert ? b.CreateAnd(result, test(second)) : b.CreateOr(result, test(second));
    }
    return result;
  }

  // A 128-bit integer crosses the runtime interface as C passes it on x86-64, which clang writes in the IR
  // of the runtime: an argument as two 64-bit ones, low then high, a result as a pair of them.
  static Value *joined(IRBuilder<> &b, Value *pair) {
    Value *low = b.CreateZExt(b.CreateExtractValue(pair, 0), b.getInt128Ty());
    Value *high = b.CreateZExt(b.CreateExtractValue(pair, 1), b.getInt128Ty());
    return b.CreateOr(b.CreateShl(high, 64), low);
  }

  // fptosi, fptoui: as LLVM's x86-64 back end legalises them, which decides what an out-of-range value
  // gives (undefined in C): the conversion to the narrowest signed integer of the machine (SSE has 32
  // and 64 bits, x87 also 16) that holds every value of the result, then truncated; to 32 bits
  // unsigned the routine that truncates the 64-bit conversion, to 64 bits unsigned its own routine.
  // x87's 16-bit conversion gives its indefinite integer, -2^15, out of range: the 32-bit conversion,
  // replaced by that when it is not a 16-bit value. Wider than 64 bits (up to 128): the 128-bit routine
  // of the same signedness (`__fixdfti`, `__fixunsdfti`), then truncated, as the back end promotes them.
  Value *toInteger(IRBuilder<> &b, Value *x, IntegerType *to, bool isSigned, const Instruction *at) {
    unsigned width = to->getBitWidth();
    if (width > 128) {
      notHandled("a conversion from " + described(x->getType()) + " to `" + printedType(to) + "`", at);
    }
    if (width > 64) {
      std::string name = std::string("__fix") + (isSigned ? "" : "uns") + mode(x->getType()) + "ti";
      Type *pair = StructType::get(b.getInt64Ty(), b.getInt64Ty());
      return b.CreateTrunc(joined(b, call(b, name, pair, {x}, at)), to);
    }
    unsigned signedWidth = isSigned ? width : width + 1;
    bool narrow = signedWidth <= 32 || (!isSigned && width == 32);
    bool unsignedRoutine = !isSigned && (width == 32 || width == 64);
    std::string name =
        std::string("__fix") + (unsignedRoutine ? "uns" : "") + mode(x->getType()) + (narrow ? "si" : "di");
    Value *result = call(b, name, narrow ? b.getInt32Ty() : b.getInt64Ty(), {x}, at);
    if (x->getType()->isX86_FP80Ty() && signedWidth <= 16) {
      Value *fits = b.CreateICmpULT(b.CreateAdd(result, b.getInt32(0x8000)), b.getInt32(0x10000));
      result = b.CreateSelect(fits, result, b.getInt32(0xFFFF8000));
    }
    return b.CreateTrunc(result, to);
  }

  // sitofp, uitofp: extended to 32, 64 or 128 bits, then the routine.
  Value *fromInteger(IRBuilder<> &b, Value *x, Type *to, bool isSigned, const Instruction *at) {
    unsigned width = x->getType()->getIntegerBitWidth();
    if (width > 128) {
      notHandled("a conversion from `" + printedType(x->getType()) + "` to " + described(to), at);
    }
    Type *wide = width <= 32 ? b.getInt32Ty() : width <= 64 ? b.getInt64Ty() : b.getInt128Ty();
    Value *extended = isSigned ? b.CreateSExt(x, wide) : b.CreateZExt(x, wide);
    std::string name = std::string("__float") + (isSigned ? "" : "un") +
                       (width <= 32   ? "si"
                        : width <= 64 ? "di"
                                      : "ti") +
                       mode(to);
    if (width > 64) {
      Value *low = b.CreateTrunc(extended, b.getInt64Ty());
      Value *high = b.CreateTrunc(b.CreateLShr(extended, 64), b.getInt64Ty());
      return call(b, name, to, {low, high}, at);
    }
    return call(b, name, to, {extended}, at);
  }

  // llvm.is.fpclass(x, mask) on the bits of x: bit 0 signalling NaN, 1 quiet NaN, 2 -inf, 3 -normal,
  // 4 -subnormal, 5 -0, 6 +0, 7 +subnormal, 8 +normal, 9 +inf.
  Value *fpClass(IRBuilder<> &b, Value *x, uint64_t mask) {
    Type *type = x->getType();
    const fltSemantics &semantics = type->getFltSemantics();
    unsigned width = type->getPrimitiveSizeInBits();
    unsigned fraction = APFloat::semanticsPrecision(semantics) - 1;
    Value *value = bits(b, x);
    auto constant = [&](const APInt &v) { return ConstantInt::get(value->getType(), v); };
    APInt sign = APInt::getSignMask(width);
    APInt infinity = APInt::getBitsSet(width, fraction, width - 1);
    APInt quiet = APInt::getOneBitSet(width, fraction - 1);
    Value *magnitude = b.CreateAnd(value, constant(~sign));
    Value *negative = b.CreateICmpSLT(value, constant(APInt::getZero(width)));
    Value *positive = b.CreateNot(negative);
    Value *isInfinity = b.CreateICmpEQ(magnitude, constant(infinity));
    Value *isNaN = b.CreateICmpUGT(magnitude, constant(infinity));
    Value *isQuiet = b.CreateICmpUGE(magnitude, constant(infinity | quiet));
    Value *isZero = b.CreateICmpEQ(magnitude, constant(APInt::getZero(width)));
    Value *isSubnormal = b.CreateAnd(b.CreateNot(isZero),
                                     b.CreateICmpULT(magnitude, constant(APInt::getOneBitSet(width, fraction))));
    Value *isNormal = b.CreateAnd(b.CreateICmpUGE(magnitude, constant(APInt::getOneBitSet(width, fraction))),
                                  b.CreateICmpULT(magnitude, constant(infinity)));
    if (type->isX86_FP80Ty()) {
      // x87's explicit integer bit (bit 63): set in every normal number, which has an exponent from 1 to
      // 0x7FFE (bits 64 to 78). Without it (an unnormal, a pseudo-infinity, a pseudo-NaN), or with it and
      // a zero exponent (a pseudo-denormal), an encoding is in no class, as LLVM's x86 expansion of
      // is.fpclass decides class by class; no arithmetic produces one.
      APInt exponentOne = APInt::getOneBitSet(width, 64);
      Value *integer = b.CreateICmpNE(b.CreateAnd(value, constant(APInt::getOneBitSet(width, 63))),
                                      constant(APInt::getZero(width)));
      isNormal = b.CreateAnd(b.CreateICmpULT(b.CreateSub(magnitude, constant(exponentOne)),
                                             constant(APInt::getBitsSet(width, 64, 79) - exponentOne)),
                             integer);
    }
    Value *classes[10] = {
        b.CreateAnd(isNaN, b.CreateNot(isQuiet)), isQuiet,
        b.CreateAnd(isInfinity, negative),        b.CreateAnd(isNormal, negative),
        b.CreateAnd(isSubnormal, negative),       b.CreateAnd(isZero, negative),
        b.CreateAnd(isZero, positive),            b.CreateAnd(isSubnormal, positive),
        b.CreateAnd(isNormal, positive),          b.CreateAnd(isInfinity, positive),
    };
    Value *result = b.getFalse();
    for (unsigned i = 0; i < 10; ++i) {
      if (mask & (uint64_t(1) << i)) {
        result = b.CreateOr(result, classes[i]);
      }
    }
    return result;
  }

  Value *lowerIntrinsic(IntrinsicInst *call) {
    IRBuilder<> b(call);
    Intrinsic::ID id = call->getIntrinsicID();
    std::string name = call->getCalledFunction()->getName().str();
    if (isa<ConstrainedFPIntrinsic>(call)) {
      notHandled("the constrained floating-point intrinsic @" + name +
                     " (floating-point state other than the default: `#pragma STDC FENV_ACCESS`)",
                 call);
    }
    if (libraryIntrinsic(id)) {
      notHandled("the intrinsic @" + name + ", a C library function (it comes with the C library)", call);
    }
    Value *x = call->arg_size() > 0 ? call->getArgOperand(0) : nullptr;
    Type *type = x && x->getType()->isFloatingPointTy() ? x->getType() : call->getType();
    if (type->isFloatingPointTy() && !computable(type)) {
      refuseType(type, call);
    }
    auto unary = [&](const char *base) { return this->call(b, base + libm(type), type, {x}, call); };
    Value *replacement = nullptr;
    switch (id) {
    case Intrinsic::fabs:
      replacement = b.CreateBitCast(b.CreateAnd(bits(b, x), ConstantInt::get(bitsOf(type), ~signBit(type))), type);
      break;
    case Intrinsic::copysign: {
      Value *magnitude = b.CreateAnd(bits(b, x), ConstantInt::get(bitsOf(type), ~signBit(type)));
      Value *sign = b.CreateAnd(bits(b, call->getArgOperand(1)), ConstantInt::get(bitsOf(type), signBit(type)));
      replacement = b.CreateBitCast(b.CreateOr(magnitude, sign), type);
      break;
    }
    case Intrinsic::is_fpclass:
      replacement = fpClass(b, x, cast<ConstantInt>(call->getArgOperand(1))->getZExtValue());
      break;
    case Intrinsic::arithmetic_fence:
      replacement = x;
      break;
    case Intrinsic::canonicalize: // as LLVM expands it: a multiplication by 1, which quiets a signalling NaN
      replacement = lowerMade(b.CreateFMul(x, ConstantFP::get(type, 1.0)));
      break;
    case Intrinsic::sqrt:
      replacement = unary("sqrt");
      break;
    case Intrinsic::floor:
      replacement = unary("floor");
      break;
    case Intrinsic::ceil:
      replacement = unary("ceil");
      break;
    case Intrinsic::trunc:
      replacement = unary("trunc");
      break;
    case Intrinsic::round:
      replacement = unary("round");
      break;
    case Intrinsic::roundeven:
      replacement = unary("roundeven");
      break;
    case Intrinsic::rint:
      replacement = unary("rint");
      break;
    case Intrinsic::nearbyint:
      replacement = unary("nearbyint");
      break;
    case Intrinsic::fma:
      replacement = this->call(b, "fma" + libm(type), type, {x, call->getArgOperand(1), call->getArgOperand(2)}, call);
      break;
    case Intrinsic::fmuladd: {
      // Unfused, as x86-64 without FMA computes it natively.
      Value *product = lowerMade(b.CreateFMul(x, call->getArgOperand(1)));
      replacement = lowerMade(b.CreateFAdd(product, call->getArgOperand(2)));
      break;
    }
    case Intrinsic::minnum:
    case Intrinsic::maxnum:
      replacement = this->call(b, (id == Intrinsic::minnum ? "fmin" : "fmax") + libm(type), type,
                               {x, call->getArgOperand(1)}, call);
      break;
    case Intrinsic::minimum:
    case Intrinsic::maximum: {
      // LangRef: a NaN if either is one; otherwise the lesser (greater), with -0 less than +0. Equal
      // values differ at most in the sign of a zero, which `or` (`and`) of the bits decides.
      Value *y = call->getArgOperand(1);
      bool minimum = id == Intrinsic::minimum;
      Value *unordered = lowerMade(b.CreateFCmpUNO(x, y));
      Value *nan = lowerMade(b.CreateFAdd(x, y));
      Value *before = lowerMade(minimum ? b.CreateFCmpOLT(x, y) : b.CreateFCmpOGT(x, y));
      Value *equal = lowerMade(b.CreateFCmpOEQ(x, y));
      Value *zeros = b.CreateBitCast(minimum ? b.CreateOr(bits(b, x), bits(b, y)) : b.CreateAnd(bits(b, x), bits(b, y)),
                                     type);
      replacement = b.CreateSelect(unordered, nan, b.CreateSelect(before, x, b.CreateSelect(equal, zeros, y)));
      break;
    }
    case Intrinsic::lrint:
    case Intrinsic::llrint:
    case Intrinsic::lround:
    case Intrinsic::llround: {
      Value *rounded = unary(id == Intrinsic::lrint || id == Intrinsic::llrint ? "rint" : "round");
      replacement = toInteger(b, rounded, cast<IntegerType>(call->getType()), true, call);
      break;
    }
    case Intrinsic::powi: {
      Value *n = call->getArgOperand(1);
      if (!n->getType()->isIntegerTy(32)) {
        notHandled("the intrinsic @" + name + " with an exponent that is not 32 bits wide", call);
      }
      replacement = this->call(b, "__powi" + mode(type) + "2", type, {x, n}, call);
      break;
    }
    default:
      notHandled("the floating-point intrinsic @" + name, call);
    }
    replace(call, replacement);
    return replacement;
  }

  // Lowers `inst` and returns what replaces it (itself if nothing does).
  Value *lower(Instruction *inst) {
    if (vectorOperation(*inst)) {
      return scalarise(inst);
    }
    IRBuilder<> b(inst);
    Value *replacement = nullptr;
    auto operandType = [&] { return inst->getOperand(0)->getType(); };
    auto check = [&](const Type *type) {
      if (!computable(type)) {
        refuseType(type, inst);
      }
    };
    switch (inst->getOpcode()) {
    case Instruction::FNeg: {
      Type *type = inst->getType();
      Value *flipped = b.CreateXor(bits(b, inst->getOperand(0)), ConstantInt::get(bitsOf(type), signBit(type)));
      replacement = b.CreateBitCast(flipped, type);
      check(type);
      break;
    }
    case Instruction::FAdd:
    case Instruction::FSub:
    case Instruction::FMul:
    case Instruction::FDiv: {
      Type *type = inst->getType();
      check(type);
      const char *op = inst->getOpcode() == Instruction::FAdd   ? "add"
                       : inst->getOpcode() == Instruction::FSub ? "sub"
                       : inst->getOpcode() == Instruction::FMul ? "mul"
                                                                : "div";
      replacement = call(b, std::string("__") + op + mode(type) + "3", type, {inst->getOperand(0), inst->getOperand(1)},
                         inst);
      break;
    }
    case Instruction::FRem:
      notHandled("the remainder of floating-point values (`frem`, fmod), which comes with the C library", inst);
    case Instruction::FCmp:
      check(operandType());
      replacement =
          compare(b, cast<FCmpInst>(inst)->getPredicate(), inst->getOperand(0), inst->getOperand(1), inst);
      break;
    case Instruction::FPExt:
    case Instruction::FPTrunc: {
      Type *from = operandType(), *to = inst->getType();
      check(from);
      check(to);
      std::string name = std::string(inst->getOpcode() == Instruction::FPExt ? "__extend" : "__trunc") + mode(from) +
                         mode(to) + "2";
      replacement = call(b, name, to, {inst->getOperand(0)}, inst);
      break;
    }
    case Instruction::FPToSI:
    case Instruction::FPToUI:
      check(operandType());
      replacement = toInteger(b, inst->getOperand(0), cast<IntegerType>(inst->getType()),
                              inst->getOpcode() == Instruction::FPToSI, inst);
      break;
    case Instruction::SIToFP:
    case Instruction::UIToFP:
      check(inst->getType());
      replacement =
          fromInteger(b, inst->getOperand(0), inst->getType(), inst->getOpcode() == Instruction::SIToFP, inst);
      break;
    case Instruction::AtomicRMW:
      if (cast<AtomicRMWInst>(inst)->isFloatingPointOperation()) {
        notHandled("an atomic floating-point update", inst);
      }
      return inst;
    case Instruction::Call:
      if (auto *intrinsic = dyn_cast<IntrinsicInst>(inst); intrinsic && !isa<DbgInfoIntrinsic>(intrinsic)) {
        return lowerIntrinsic(intrinsic);
      }
      return inst;
    default:
      return inst; // moves values: rewritten by floatsToIntegers
    }
    replace(inst, replacement);
    return replacement;
  }

  Module &module;
  Runtime *runtime;
};

bool usesFloat(const Module &module) {
  for (const GlobalVariable &global : module.globals()) {
    if (hasFloat(global.getValueType())) {
      return true;
    }
  }
  for (const Function &function : module) {
    if (hasFloat(function.getFunctionType())) {
      return true;
    }
    for (const Instruction &inst : instructions(function)) {
      if (hasFloat(inst.getType())) {
        return true;
      }
      for (const Use &operand : inst.operands()) {
        if (hasFloat(operand->getType())) {
          return true;
        }
      }
      if (const auto *alloca = dyn_cast<AllocaInst>(&inst); alloca && hasFloat(alloca->getAllocatedType())) {
        return true;
      }
      if (const auto *gep = dyn_cast<GetElementPtrInst>(&inst); gep && hasFloat(gep->getSourceElementType())) {
        return true;
      }
      if (const auto *call = dyn_cast<CallBase>(&inst); call && hasFloat(call->getFunctionType())) {
        return true;
      }
    }
  }
  return false;
}

// Step 3: each floating-point type is the integer of its width. Memory must not move: every rewritten type
// that has a size has the same size, store size and alignments as the type it replaces, and a structure
// every field at the same offset (an internal error otherwise). On x86-64 the integer of 80 bits is
// aligned as the next wider integer the data layout names, i128, to 16 bytes, as x86_fp80 is.
class FloatBits : public ValueMapTypeRemapper {
public:
  explicit FloatBits(const DataLayout &layout) : layout(layout) {}

  Type *remapType(Type *type) override {
    Type *mapped = rewritten(type);
    if (mapped != type && type->isSized()) {
      checkLayout(type, mapped);
    }
    return mapped;
  }

private:
  void checkLayout(Type *from, Type *to) {
    if (!checked.insert(from).second) {
      return;
    }
    bool same = layout.getTypeSizeInBits(from) == layout.getTypeSizeInBits(to) &&
                layout.getTypeStoreSize(from) == layout.getTypeStoreSize(to) &&
                layout.getTypeAllocSize(from) == layout.getTypeAllocSize(to) &&
                layout.getABITypeAlign(from) == layout.getABITypeAlign(to) &&
                layout.getPrefTypeAlign(from) == layout.getPrefTypeAlign(to);
    if (auto *structure = dyn_cast<StructType>(from); same && structure) {
      const StructLayout *before = layout.getStructLayout(structure);
      const StructLayout *after = layout.getStructLayout(cast<StructType>(to));
      for (unsigned i = 0; i < structure->getNumElements(); ++i) {
        same &= before->getElementOffset(i) == after->getElementOffset(i);
      }
    }
    if (!same) {
      fail("internal: rewriting " + printedType(from) + " as " + printedType(to) +
           " changes its size, alignment or field offsets in the data layout `" +
           layout.getStringRepresentation() + "`");
    }
  }

  Type *rewritten(Type *type) {
    if (!hasFloat(type)) {
      return type;
    }
    if (type->isFloatingPointTy()) {
      return IntegerType::get(type->getContext(), type->getPrimitiveSizeInBits());
    }
    if (auto *vector = dyn_cast<VectorType>(type)) {
      return VectorType::get(remapType(vector->getElementType()), vector->getElementCount());
    }
    if (auto *array = dyn_cast<ArrayType>(type)) {
      return ArrayType::get(remapType(array->getElementType()), array->getNumElements());
    }
    if (auto *function = dyn_cast<FunctionType>(type)) {
      std::vector<Type *> params;
      for (Type *param : function->params()) {
        params.push_back(remapType(param));
      }
      return FunctionType::get(remapType(function->getReturnType()), params, function->isVarArg());
    }
    auto *structure = cast<StructType>(type);
    if (auto found = structs.find(structure); found != structs.end()) {
      return found->second;
    }
    std::vector<Type *> elements;
    for (Type *element : structure->elements()) {
      elements.push_back(remapType(element));
    }
    StructType *mapped = structure->isLiteral()
                             ? StructType::get(type->getContext(), elements, structure->isPacked())
                             : StructType::create(type->getContext(), elements, structure->getName(),
                                                  structure->isPacked());
    structs[structure] = mapped;
    return mapped;
  }

  const DataLayout &layout;
  std::map<StructType *, StructType *> structs;
  std::set<Type *> checked;
};

// Constants of floating-point type become their bit patterns.
class ConstantBits : public ValueMaterializer {
public:
  explicit ConstantBits(FloatBits &types) : types(types) {}

  Value *materialize(Value *value) override {
    auto *constant = dyn_cast<Constant>(value);
    if (!constant || isa<GlobalValue>(constant)) {
      return nullptr;
    }
    return map(constant);
  }

  // Not cached here: replacing a global destroys the constants that use it and makes new ones, possibly
  // at the same addresses (the ValueMapper's own map is kept up to date by value handles).
  Constant *map(Constant *constant) { return isa<GlobalValue>(constant) ? constant : mapNew(constant); }

private:
  Constant *mapNew(Constant *constant) {
    Type *type = types.remapType(constant->getType());
    if (auto *fp = dyn_cast<ConstantFP>(constant)) {
      return ConstantInt::get(type, fp->getValueAPF().bitcastToAPInt());
    }
    if (auto *data = dyn_cast<ConstantDataSequential>(constant)) {
      if (!data->getElementType()->isFloatingPointTy()) {
        return constant;
      }
      Type *element = types.remapType(data->getElementType());
      if (isa<ConstantDataArray>(data)) {
        return ConstantDataArray::getRaw(data->getRawDataValues(), data->getNumElements(), element);
      }
      return ConstantDataVector::getRaw(data->getRawDataValues(), data->getNumElements(), element);
    }
    if (isa<PoisonValue>(constant)) {
      return PoisonValue::get(type);
    }
    if (isa<UndefValue>(constant)) {
      return UndefValue::get(type);
    }
    if (isa<ConstantAggregateZero>(constant)) {
      return ConstantAggregateZero::get(type);
    }
    std::vector<Constant *> operands;
    bool changed = type != constant->getType();
    for (Use &operand : constant->operands()) {
      Constant *mapped = map(cast<Constant>(operand.get()));
      changed |= mapped != operand.get();
      operands.push_back(mapped);
    }
    auto *expression = dyn_cast<ConstantExpr>(constant);
    Type *source = nullptr;
    if (auto *gep = dyn_cast<GEPOperator>(constant)) {
      source = types.remapType(gep->getSourceElementType());
      changed |= source != gep->getSourceElementType();
    }
    if (!changed) {
      return constant;
    }
    if (expression) {
      if (expression->getOpcode() == Instruction::BitCast && operands[0]->getType() == type) {
        return operands[0];
      }
      if (auto *gep = dyn_cast<GEPOperator>(constant)) {
        // getWithOperands returns the expression itself when only the source element type changed.
        return ConstantExpr::getGetElementPtr(source, operands[0], ArrayRef<Constant *>(operands).drop_front(),
                                              gep->isInBounds(), gep->getInRangeIndex());
      }
      return expression->getWithOperands(operands, type, false, source);
    }
    if (isa<ConstantStruct>(constant)) {
      return ConstantStruct::get(cast<StructType>(type), operands);
    }
    if (isa<ConstantArray>(constant)) {
      return ConstantArray::get(cast<ArrayType>(type), operands);
    }
    if (isa<ConstantVector>(constant)) {
      return ConstantVector::get(operands);
    }
    fail("internal: the floating-point constant " + printedType(constant->getType()) + " cannot be rewritten");
  }

  FloatBits &types;
};

// Attributes with the types they name rewritten, and without `nofpclass`, which only floating-point
// values may carry.
AttributeList mappedAttributes(LLVMContext &context, AttributeList attributes, unsigned count, FloatBits &types) {
  const Attribute::AttrKind typed[] = {Attribute::ByVal,    Attribute::StructRet,    Attribute::ByRef,
                                       Attribute::InAlloca, Attribute::Preallocated, Attribute::ElementType};
  std::vector<AttributeSet> params;
  for (unsigned i = 0; i < count; ++i) {
    AttrBuilder builder(context, attributes.getParamAttrs(i));
    builder.removeAttribute(Attribute::NoFPClass);
    for (Attribute::AttrKind kind : typed) {
      if (Type *type = builder.getTypeAttr(kind)) {
        builder.addTypeAttr(kind, types.remapType(type));
      }
    }
    params.push_back(AttributeSet::get(context, builder));
  }
  AttrBuilder result(context, attributes.getRetAttrs());
  result.removeAttribute(Attribute::NoFPClass);
  return AttributeList::get(context, attributes.getFnAttrs(), AttributeSet::get(context, result), params);
}

} // namespace

bool lowerFloatOperations(Module &module, const std::string &runtimePath) {
  if (!usesFloat(module)) {
    return false;
  }
  Runtime runtime(module, runtimePath);
  std::vector<Function *> functions;
  for (Function &function : module) {
    if (!function.isDeclaration()) {
      functions.push_back(&function);
    }
  }
  Lowerer program(module, &runtime);
  for (Function *function : functions) {
    program.lowerFunction(*function);
  }
  bool linked = runtime.link();
  Lowerer inRuntime(module, nullptr);
  for (Function &function : module) {
    if (!function.isDeclaration() && function.getName().starts_with(runtimePrefix)) {
      inRuntime.lowerFunction(function);
    }
  }
  return linked;
}

void floatsToIntegers(Module &module) {
  if (!usesFloat(module)) {
    return;
  }
  LLVMContext &context = module.getContext();
  // What is left of the floating-point intrinsics is their declarations.
  for (Function &function : make_early_inc_range(module)) {
    if (function.isIntrinsic() && hasFloat(function.getFunctionType())) {
      if (!function.use_empty()) {
        fail("internal: the floating-point intrinsic @" + function.getName().str() + " is still called");
      }
      function.eraseFromParent();
    }
  }
  // Fast-math flags and `nofpclass` mean nothing on integers.
  for (Function &function : module) {
    for (Instruction &inst : instructions(function)) {
      if (isa<FPMathOperator>(inst)) {
        inst.copyFastMathFlags(FastMathFlags());
      }
    }
  }

  FloatBits types(module.getDataLayout());
  ConstantBits constants(types);
  ValueToValueMapTy map;

  // Functions whose type has a floating-point value get a new function of the rewritten type, which takes
  // the body over; their uses are pointers, so they can be replaced at once, and their arguments are
  // mapped to the new ones.
  std::vector<Function *> retyped;
  for (Function &function : module) {
    if (hasFloat(function.getFunctionType())) {
      retyped.push_back(&function);
    }
  }
  for (Function *old : retyped) {
    auto *type = cast<FunctionType>(types.remapType(old->getFunctionType()));
    Function *function = Function::Create(type, old->getLinkage(), old->getAddressSpace(), "");
    module.getFunctionList().insert(old->getIterator(), function);
    function->copyAttributesFrom(old);
    function->setAttributes(mappedAttributes(context, old->getAttributes(), old->arg_size(), types));
    function->setSubprogram(old->getSubprogram());
    old->setSubprogram(nullptr);
    SmallVector<std::pair<unsigned, MDNode *>, 4> metadata;
    old->getAllMetadata(metadata);
    for (auto &[kind, node] : metadata) {
      function->setMetadata(kind, node);
    }
    function->takeName(old);
    function->splice(function->begin(), old);
    auto arg = function->arg_begin();
    for (Argument &oldArg : old->args()) {
      arg->takeName(&oldArg);
      map[&oldArg] = &*arg;
      ++arg;
    }
    old->replaceAllUsesWith(function);
  }

  // Globals likewise; every initializer is rewritten.
  std::vector<GlobalVariable *> globals;
  for (GlobalVariable &global : module.globals()) {
    globals.push_back(&global);
  }
  for (GlobalVariable *old : globals) {
    Constant *initializer = old->hasInitializer() ? constants.map(old->getInitializer()) : nullptr;
    if (!hasFloat(old->getValueType())) {
      if (initializer && initializer != old->getInitializer()) {
        old->setInitializer(initializer);
      }
      continue;
    }
    auto *global = new GlobalVariable(module, types.remapType(old->getValueType()), old->isConstant(),
                                      old->getLinkage(), initializer, "", old, old->getThreadLocalMode(),
                                      old->getAddressSpace(), old->isExternallyInitialized());
    global->copyAttributesFrom(old);
    SmallVector<std::pair<unsigned, MDNode *>, 4> metadata;
    old->getAllMetadata(metadata);
    for (auto &[kind, node] : metadata) {
      global->setMetadata(kind, node);
    }
    global->takeName(old);
    old->replaceAllUsesWith(global);
    old->eraseFromParent();
  }

  // Every body, in place: types of instructions and calls, constants.
  ValueMapper mapper(map, RF_IgnoreMissingLocals | RF_NoModuleLevelChanges, &types, &constants);
  for (Function &function : module) {
    if (function.isDeclaration()) {
      continue;
    }
    for (Instruction &inst : instructions(function)) {
      if (auto *call = dyn_cast<CallBase>(&inst)) {
        call->setAttributes(mappedAttributes(context, call->getAttributes(), call->arg_size(), types));
      }
    }
    mapper.remapFunction(function);
  }
  // The old functions go only now: the map is keyed by their arguments.
  for (Function *old : retyped) {
    old->eraseFromParent();
  }
  // A bitcast between a floating-point value and an integer is now one from an integer to itself.
  for (Function &function : module) {
    for (Instruction &inst : make_early_inc_range(instructions(function))) {
      if (auto *cast = dyn_cast<BitCastInst>(&inst); cast && cast->getSrcTy() == cast->getDestTy()) {
        cast->replaceAllUsesWith(cast->getOperand(0));
        cast->eraseFromParent();
      }
    }
  }
}

bool foldConstantCalls(Module &module) {
  TargetLibraryInfoImpl libraryInfo{Triple(module.getTargetTriple())};
  TargetLibraryInfo library(libraryInfo);
  bool folded = false;
  for (Function &function : module) {
    // A rotation's angle computed by a function of the program on constants: inlined, a few levels deep,
    // so that what is left are runtime calls on constants.
    for (unsigned depth = 0; depth < 8; ++depth) {
      std::vector<CallInst *> angles;
      for (Instruction &inst : instructions(function)) {
        std::optional<Marker> marker = markerCall(&inst);
        if (marker && marker->kind == MarkerKind::Rotation) {
          auto *angle = dyn_cast<CallInst>(cast<CallInst>(inst).getArgOperand(1));
          Function *callee = angle ? angle->getCalledFunction() : nullptr;
          if (callee && !callee->isDeclaration() && callee->onlyReadsMemory()) {
            angles.push_back(angle);
          }
        }
      }
      bool inlined = false;
      for (CallInst *angle : angles) {
        InlineFunctionInfo info;
        inlined |= InlineFunction(*angle, info).isSuccess();
      }
      if (!inlined) {
        break;
      }
      simplifyDeep(function);
      folded = true;
    }
    for (bool changed = true; changed;) {
      changed = false;
      for (Instruction &inst : make_early_inc_range(instructions(function))) {
        auto *call = dyn_cast<CallInst>(&inst);
        Function *callee = call ? call->getCalledFunction() : nullptr;
        if (!callee || callee->isDeclaration() || !callee->getName().starts_with(runtimePrefix) ||
            !callee->onlyReadsMemory() || call->getType()->isVoidTy()) {
          continue;
        }
        SmallVector<Constant *, 4> args;
        for (Value *arg : call->args()) {
          if (auto *constant = dyn_cast<Constant>(arg)) {
            args.push_back(constant);
          }
        }
        if (args.size() != call->arg_size()) {
          continue;
        }
        Evaluator evaluator(module.getDataLayout(), &library);
        Constant *result = nullptr;
        if (!evaluator.EvaluateFunction(callee, result, args) || !result ||
            !evaluator.getMutatedInitializers().empty()) {
          continue;
        }
        call->replaceAllUsesWith(result);
        call->eraseFromParent();
        changed = true;
      }
      if (changed) {
        folded = true;
        simplifyDeep(function);
      }
    }
  }
  return folded;
}

void checkNoFloatingPoint(const Module &module, const char *stage) {
  auto check = [&](const Type *type, const Value *at) {
    if (hasFloat(type)) {
      fail(std::string("internal: the floating-point type ") + printedType(type) + " is left " + stage + ", at " +
           locationOf(at));
    }
  };
  std::function<void(const Constant *, const Value *, std::set<const Constant *> &)> constant =
      [&](const Constant *c, const Value *at, std::set<const Constant *> &seen) {
        if (!seen.insert(c).second || isa<GlobalValue>(c)) {
          return;
        }
        check(c->getType(), at);
        if (const auto *gep = dyn_cast<GEPOperator>(c)) {
          check(gep->getSourceElementType(), at);
        }
        for (const Use &operand : c->operands()) {
          constant(cast<Constant>(operand.get()), at, seen);
        }
      };
  std::set<const Constant *> seen;
  for (const GlobalVariable &global : module.globals()) {
    check(global.getValueType(), &global);
    if (global.hasInitializer()) {
      constant(global.getInitializer(), &global, seen);
    }
  }
  for (const Function &function : module) {
    if (function.isDeclaration() && function.use_empty()) {
      continue;
    }
    check(function.getFunctionType(), &function);
    for (const Instruction &inst : instructions(function)) {
      check(inst.getType(), &inst);
      for (const Use &operand : inst.operands()) {
        check(operand->getType(), &inst);
        if (const auto *c = dyn_cast<Constant>(operand.get())) {
          constant(c, &inst, seen);
        }
      }
      if (const auto *alloca = dyn_cast<AllocaInst>(&inst)) {
        check(alloca->getAllocatedType(), &inst);
      }
      if (const auto *gep = dyn_cast<GetElementPtrInst>(&inst)) {
        check(gep->getSourceElementType(), &inst);
      }
      if (const auto *call = dyn_cast<CallBase>(&inst)) {
        check(call->getFunctionType(), &inst);
      }
    }
  }
}

} // namespace bennett
