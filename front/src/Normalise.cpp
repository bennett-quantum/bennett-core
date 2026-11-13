#include "Normalise.h"

#include <llvm/Analysis/TargetTransformInfo.h>
#include <llvm/CodeGen/IntrinsicLowering.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>
#include <llvm/Analysis/AliasAnalysis.h>
#include <llvm/Passes/PassBuilder.h>
#include <llvm/Transforms/InstCombine/InstCombine.h>
#include <llvm/Transforms/Scalar/EarlyCSE.h>
#include <llvm/Transforms/Utils/LowerMemIntrinsics.h>

#include <vector>

using namespace llvm;

namespace bennett {

void optimise(Module &module, PassBuilder &builder, ModuleAnalysisManager &analyses) {
  for (Function &function : module) {
    if (function.hasFnAttribute(Attribute::OptimizeNone)) {
      function.removeFnAttr(Attribute::OptimizeNone);
      function.removeFnAttr(Attribute::NoInline);
    }
  }
  ModulePassManager passes = builder.buildPerModuleDefaultPipeline(OptimizationLevel::O2);
  passes.run(module, analyses);
}

void simplify(Function &function) {
  LoopAnalysisManager loops;
  FunctionAnalysisManager functions;
  CGSCCAnalysisManager sccs;
  ModuleAnalysisManager modules;
  PassBuilder builder;
  functions.registerPass([&] { return builder.buildDefaultAAPipeline(); });
  builder.registerModuleAnalyses(modules);
  builder.registerCGSCCAnalyses(sccs);
  builder.registerFunctionAnalyses(functions);
  builder.registerLoopAnalyses(loops);
  builder.crossRegisterProxies(loops, functions, sccs, modules);
  FunctionPassManager passes;
  passes.addPass(InstCombinePass());
  passes.addPass(EarlyCSEPass());
  passes.addPass(InstCombinePass());
  passes.run(function, functions);
}

namespace {

// a < b ? a : b and the like.
Value *minMax(IRBuilder<> &b, CmpInst::Predicate pred, Value *x, Value *y) {
  return b.CreateSelect(b.CreateICmp(pred, x, y), x, y);
}

// fshl(a, b, c) = (a << (c mod w)) | (b >> (w - c mod w)); the device program's shift by the full width
// gives 0, which is exactly the c mod w = 0 case. fshr is the mirror image.
Value *funnel(IRBuilder<> &b, bool left, Value *hi, Value *lo, Value *amount) {
  auto *type = cast<IntegerType>(hi->getType());
  Value *width = ConstantInt::get(type, type->getBitWidth());
  Value *shift = b.CreateURem(amount, width);
  Value *rest = b.CreateSub(width, shift);
  if (left) {
    return b.CreateOr(b.CreateShl(hi, shift), b.CreateLShr(lo, rest));
  }
  return b.CreateOr(b.CreateShl(hi, rest), b.CreateLShr(lo, shift));
}

} // namespace

void lowerIntrinsics(Module &module) {
  const DataLayout &layout = module.getDataLayout();
  TargetTransformInfo costs(layout);
  IntrinsicLowering generic(layout);
  for (Function &function : module) {
    std::vector<IntrinsicInst *> calls;
    for (Instruction &inst : instructions(function)) {
      if (auto *call = dyn_cast<IntrinsicInst>(&inst)) {
        calls.push_back(call);
      }
    }
    for (IntrinsicInst *call : calls) {
      IRBuilder<> b(call);
      Value *replacement = nullptr;
      switch (call->getIntrinsicID()) {
      case Intrinsic::lifetime_start:
      case Intrinsic::lifetime_end:
      case Intrinsic::assume:
      case Intrinsic::experimental_noalias_scope_decl:
      case Intrinsic::dbg_declare:
      case Intrinsic::dbg_value:
      case Intrinsic::dbg_label:
      case Intrinsic::donothing:
        call->eraseFromParent();
        continue;
      case Intrinsic::smax:
        replacement = minMax(b, CmpInst::ICMP_SGT, call->getArgOperand(0), call->getArgOperand(1));
        break;
      case Intrinsic::smin:
        replacement = minMax(b, CmpInst::ICMP_SLT, call->getArgOperand(0), call->getArgOperand(1));
        break;
      case Intrinsic::umax:
        replacement = minMax(b, CmpInst::ICMP_UGT, call->getArgOperand(0), call->getArgOperand(1));
        break;
      case Intrinsic::umin:
        replacement = minMax(b, CmpInst::ICMP_ULT, call->getArgOperand(0), call->getArgOperand(1));
        break;
      case Intrinsic::abs: {
        Value *x = call->getArgOperand(0);
        Value *negative = b.CreateICmpSLT(x, ConstantInt::get(x->getType(), 0));
        replacement = b.CreateSelect(negative, b.CreateNeg(x), x);
        break;
      }
      case Intrinsic::fshl:
      case Intrinsic::fshr:
        replacement = funnel(b, call->getIntrinsicID() == Intrinsic::fshl, call->getArgOperand(0),
                             call->getArgOperand(1), call->getArgOperand(2));
        break;
      case Intrinsic::ctpop:
      case Intrinsic::ctlz:
      case Intrinsic::cttz:
      case Intrinsic::bswap:
        generic.LowerIntrinsicCall(call);
        continue;
      case Intrinsic::memset:
        expandMemSetAsLoop(cast<MemSetInst>(call));
        call->eraseFromParent();
        continue;
      case Intrinsic::memcpy:
        expandMemCpyAsLoop(cast<MemCpyInst>(call), costs);
        call->eraseFromParent();
        continue;
      case Intrinsic::memmove:
        if (expandMemMoveAsLoop(cast<MemMoveInst>(call), costs)) {
          call->eraseFromParent();
        }
        continue;
      default:
        continue; // the emitter names whatever is left as not handled
      }
      call->replaceAllUsesWith(replacement);
      call->eraseFromParent();
    }
  }
}

} // namespace bennett
