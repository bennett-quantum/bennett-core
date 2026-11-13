#include "Normalise.h"

#include "Diagnostic.h"

#include <llvm/Analysis/TargetTransformInfo.h>
#include <llvm/CodeGen/IntrinsicLowering.h>
#include <llvm/IR/AttributeMask.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>
#include <llvm/Analysis/AliasAnalysis.h>
#include <llvm/Passes/PassBuilder.h>
#include <llvm/Transforms/InstCombine/InstCombine.h>
#include <llvm/Transforms/Scalar/EarlyCSE.h>
#include <llvm/Transforms/Utils/LowerMemIntrinsics.h>

#include <functional>
#include <set>
#include <string>
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

void simplifyDeep(Function &function) {
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
  if (Error error = builder.parsePassPipeline(
          passes, "sccp,instcombine,simplifycfg,early-cse,instcombine,simplifycfg,adce,instcombine,simplifycfg")) {
    fail("internal: " + toString(std::move(error)));
  }
  passes.run(function, functions);
}

namespace {

// a < b ? a : b and the like.
Value *minMax(IRBuilder<> &b, CmpInst::Predicate pred, Value *x, Value *y) {
  return b.CreateSelect(b.CreateICmp(pred, x, y), x, y);
}

// fshl(a, b, c) = (a << s) | (b >> (w - s)) with s = c mod w, where the second term is 0 when s = 0.
// It is written ((b >> 1) >> (w - 1 - s)) so that no shift is by the full width: that would be 0 in the
// device program but poison in LLVM, and the front still runs LLVM's simplifier on lowered code. fshr is
// the mirror image.
Value *funnel(IRBuilder<> &b, bool left, Value *hi, Value *lo, Value *amount) {
  auto *type = cast<IntegerType>(hi->getType());
  Value *width = ConstantInt::get(type, type->getBitWidth());
  Value *shift = b.CreateURem(amount, width);
  Value *rest = b.CreateSub(ConstantInt::get(type, type->getBitWidth() - 1), shift);
  Value *one = ConstantInt::get(type, 1);
  if (left) {
    return b.CreateOr(b.CreateShl(hi, shift), b.CreateLShr(b.CreateLShr(lo, one), rest));
  }
  return b.CreateOr(b.CreateShl(b.CreateShl(hi, one), rest), b.CreateLShr(lo, shift));
}

// Runs `expand`, then gives every instruction it added that has no debug location the location `at`, so
// that diagnostics about the expansion point at the source.
void locate(Function &function, const DebugLoc &at, const std::function<void()> &expand) {
  std::set<const Instruction *> before;
  for (const Instruction &inst : instructions(function)) {
    before.insert(&inst);
  }
  expand();
  for (Instruction &inst : instructions(function)) {
    if (!before.count(&inst) && !inst.getDebugLoc()) {
      inst.setDebugLoc(at);
    }
  }
}

// Parameter attributes that change what an argument is, and that the device program cannot give.
const Attribute::AttrKind refusedParameterAttributes[] = {Attribute::InAlloca, Attribute::Preallocated,
                                                          Attribute::SwiftError};

void refuseParameterAttributes(const AttributeList &attributes, unsigned params, const Value *at) {
  for (unsigned i = 0; i < params; ++i) {
    for (Attribute::AttrKind kind : refusedParameterAttributes) {
      if (attributes.hasParamAttr(i, kind)) {
        notHandled("a parameter with the attribute `" + Attribute::getNameFromAttrKind(kind).str() + "`", at);
      }
    }
  }
}

// Writes through the parameter, or reading it, are no longer what the attributes promised once the callee
// reads the pointee to copy it.
void dropByVal(AttributeMask &mask) {
  mask.addAttribute(Attribute::ByVal);
  mask.addAttribute(Attribute::ReadNone);
  mask.addAttribute(Attribute::WriteOnly);
}

} // namespace

void lowerParameterAttributes(Module &module) {
  const DataLayout &layout = module.getDataLayout();
  for (Function &function : module) {
    refuseParameterAttributes(function.getAttributes(), function.arg_size(), &function);
    for (Instruction &inst : instructions(function)) {
      auto *call = dyn_cast<CallBase>(&inst);
      if (!call) {
        continue;
      }
      refuseParameterAttributes(call->getAttributes(), call->arg_size(), call);
      bool byval = false;
      for (unsigned i = 0; i < call->arg_size(); ++i) {
        if (call->isByValArgument(i)) {
          AttributeMask mask;
          dropByVal(mask);
          call->removeParamAttrs(i, mask);
          byval = true;
        }
      }
      if (byval && call->hasFnAttr(Attribute::Memory)) {
        call->setMemoryEffects(call->getMemoryEffects() | MemoryEffects::argMemOnly(ModRefInfo::Ref));
      }
    }
    if (function.isDeclaration()) {
      continue;
    }
    bool byval = false;
    for (Argument &arg : function.args()) {
      if (!arg.hasByValAttr()) {
        continue;
      }
      byval = true;
      Type *type = arg.getParamByValType();
      Align align = std::max(arg.getParamAlign().valueOrOne(), layout.getABITypeAlign(type));
      BasicBlock &entry = function.getEntryBlock();
      IRBuilder<> b(&entry, entry.getFirstInsertionPt());
      b.SetCurrentDebugLocation(entry.getFirstNonPHIOrDbg()->getDebugLoc());
      AllocaInst *copy = b.CreateAlloca(type, nullptr, arg.getName() + ".copy");
      copy->setAlignment(align);
      arg.replaceAllUsesWith(copy);
      b.CreateMemCpy(copy, align, &arg, arg.getParamAlign(), layout.getTypeAllocSize(type).getFixedValue());
      AttributeMask mask;
      dropByVal(mask);
      arg.removeAttrs(mask);
    }
    if (byval && function.hasFnAttribute(Attribute::Memory)) {
      function.setMemoryEffects(function.getMemoryEffects() | MemoryEffects::argMemOnly(ModRefInfo::Ref));
    }
  }
}

void lowerIntrinsics(Module &module, bool keepBijections) {
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
        if (keepBijections) {
          continue;
        }
        replacement = funnel(b, call->getIntrinsicID() == Intrinsic::fshl, call->getArgOperand(0),
                             call->getArgOperand(1), call->getArgOperand(2));
        break;
      case Intrinsic::bswap:
        if (keepBijections) {
          continue;
        }
        [[fallthrough]];
      case Intrinsic::ctpop:
      case Intrinsic::ctlz:
      case Intrinsic::cttz:
        locate(function, call->getDebugLoc(), [&] { generic.LowerIntrinsicCall(call); });
        continue;
      case Intrinsic::memset:
        locate(function, call->getDebugLoc(), [&] { expandMemSetAsLoop(cast<MemSetInst>(call)); });
        call->eraseFromParent();
        continue;
      case Intrinsic::memcpy:
        locate(function, call->getDebugLoc(), [&] { expandMemCpyAsLoop(cast<MemCpyInst>(call), costs); });
        call->eraseFromParent();
        continue;
      case Intrinsic::memmove: {
        bool expanded = false;
        locate(function, call->getDebugLoc(),
               [&] { expanded = expandMemMoveAsLoop(cast<MemMoveInst>(call), costs); });
        if (expanded) {
          call->eraseFromParent();
        }
        continue;
      }
      default:
        continue; // the emitter names whatever is left as not handled
      }
      call->replaceAllUsesWith(replacement);
      call->eraseFromParent();
    }
  }
}

} // namespace bennett
