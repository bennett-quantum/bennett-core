#include "Normalise.h"

#include "Diagnostic.h"

#include <llvm/Analysis/TargetTransformInfo.h>
#include <llvm/CodeGen/IntrinsicLowering.h>
#include <llvm/IR/AttributeMask.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InlineAsm.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>
#include <llvm/IR/ValueHandle.h>
#include <llvm/Analysis/AliasAnalysis.h>
#include <llvm/Passes/PassBuilder.h>
#include <llvm/Transforms/InstCombine/InstCombine.h>
#include <llvm/Transforms/Scalar/DCE.h>
#include <llvm/Transforms/Scalar/EarlyCSE.h>
#include <llvm/Transforms/Scalar/Scalarizer.h>
#include <llvm/Transforms/Utils/Local.h>
#include <llvm/Transforms/Utils/LowerMemIntrinsics.h>

#include <algorithm>
#include <functional>
#include <map>
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

namespace {

// Instructions in `module` that make or use a vector value.
size_t vectorValues(Module &module) {
  size_t count = 0;
  for (Function &function : module) {
    for (Instruction &inst : instructions(function)) {
      count += inst.getType()->isVectorTy() ||
               std::any_of(inst.op_begin(), inst.op_end(), [](const Use &use) { return use->getType()->isVectorTy(); });
    }
  }
  return count;
}

// A bitcast between a vector and an integer goes through memory, as SelectionDAG expands it, where the
// Scalarizer splits the side that is a vector. Only for elements of whole bytes: LLVM packs a vector of
// i1 into bits in memory.
void bitcastsThroughMemory(Module &module) {
  const DataLayout &layout = module.getDataLayout();
  auto wholeBytes = [&](Type *type) {
    auto *vector = dyn_cast<FixedVectorType>(type);
    return vector && layout.getTypeSizeInBits(vector->getElementType()) % 8 == 0 &&
           layout.getTypeStoreSize(vector->getElementType()) == layout.getTypeAllocSize(vector->getElementType());
  };
  for (Function &function : module) {
    std::vector<BitCastInst *> casts;
    for (Instruction &inst : instructions(function)) {
      auto *cast = dyn_cast<BitCastInst>(&inst);
      if (!cast) {
        continue;
      }
      Type *from = cast->getSrcTy(), *to = cast->getDestTy();
      if ((wholeBytes(from) && to->isIntegerTy()) || (from->isIntegerTy() && wholeBytes(to))) {
        casts.push_back(cast);
      }
    }
    for (BitCastInst *cast : casts) {
      IRBuilder<> entry(&function.getEntryBlock(), function.getEntryBlock().getFirstInsertionPt());
      AllocaInst *memory = entry.CreateAlloca(cast->getSrcTy(), nullptr, "bitcast");
      IRBuilder<> b(cast);
      b.CreateStore(cast->getOperand(0), memory);
      Value *value = b.CreateLoad(cast->getDestTy(), memory);
      value->takeName(cast);
      cast->replaceAllUsesWith(value);
      cast->eraseFromParent();
    }
  }
}

} // namespace

void scalarise(Module &module) {
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
  ScalarizerPassOptions options;
  options.ScalarizeLoadStore = true;
  options.ScalarizeVariableInsertExtract = true;
  FunctionPassManager passes;
  passes.addPass(ScalarizerPass(options));
  passes.addPass(InstCombinePass());
  passes.addPass(DCEPass());
  ModulePassManager modulePasses;
  modulePasses.addPass(createModuleToFunctionPassAdaptor(std::move(passes)));
  // Splitting a bitcast between vectors of different element counts makes bitcasts between smaller
  // vectors and integers: go round while the vectors get fewer.
  for (size_t before = vectorValues(module); before > 0;) {
    bitcastsThroughMemory(module);
    modulePasses.run(module, modules);
    modules.clear();
    size_t after = vectorValues(module);
    if (after >= before) {
      break;
    }
    before = after;
  }
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

// Replaces the `{ result, flag }` an intrinsic returns: each extractvalue of it reads its part, and any
// other use gets the aggregate rebuilt (which the emitter then refuses by name).
void replaceParts(IntrinsicInst *call, const std::vector<Value *> &parts) {
  IRBuilder<> b(call);
  Value *whole = nullptr;
  for (Use &use : make_early_inc_range(call->uses())) {
    auto *extract = dyn_cast<ExtractValueInst>(use.getUser());
    if (extract && extract->getNumIndices() == 1) {
      extract->replaceAllUsesWith(parts[extract->getIndices()[0]]);
      extract->eraseFromParent();
      continue;
    }
    if (!whole) {
      whole = PoisonValue::get(call->getType());
      for (unsigned i = 0; i < parts.size(); ++i) {
        whole = b.CreateInsertValue(whole, parts[i], i);
      }
    }
    use.set(whole);
  }
  call->eraseFromParent();
}

// x op y with an overflow flag, by definition: computed exactly in twice the width (where no sum,
// difference or product of two such values can wrap), then the flag says whether the result truncated
// to the width extends back to the exact value.
void withOverflow(IntrinsicInst *call, Instruction::BinaryOps op, bool isSigned) {
  IRBuilder<> b(call);
  auto *type = cast<IntegerType>(call->getArgOperand(0)->getType());
  Type *wide = b.getIntNTy(2 * type->getBitWidth());
  auto extend = [&](Value *v) { return isSigned ? b.CreateSExt(v, wide) : b.CreateZExt(v, wide); };
  Value *exact = b.CreateBinOp(op, extend(call->getArgOperand(0)), extend(call->getArgOperand(1)));
  Value *result = b.CreateTrunc(exact, type);
  replaceParts(call, {result, b.CreateICmpNE(extend(result), exact)});
}

// x op y clamped to the range of the type, by definition: the exact value in twice the width, clamped.
// An unsigned difference below zero clamps to zero.
Value *saturated(IRBuilder<> &b, IntrinsicInst *call, Instruction::BinaryOps op, bool isSigned) {
  Value *x = call->getArgOperand(0), *y = call->getArgOperand(1);
  auto *type = cast<IntegerType>(x->getType());
  unsigned bits = type->getBitWidth();
  if (!isSigned && op == Instruction::Sub) {
    return b.CreateSelect(b.CreateICmpULT(x, y), ConstantInt::get(type, 0), b.CreateSub(x, y));
  }
  Type *wide = b.getIntNTy(2 * bits);
  auto extend = [&](Value *v) { return isSigned ? b.CreateSExt(v, wide) : b.CreateZExt(v, wide); };
  Value *exact = b.CreateBinOp(op, extend(x), extend(y));
  APInt max = isSigned ? APInt::getSignedMaxValue(bits) : APInt::getMaxValue(bits);
  Value *high = ConstantInt::get(wide, isSigned ? max.sext(2 * bits) : max.zext(2 * bits));
  Value *clamped = b.CreateSelect(isSigned ? b.CreateICmpSGT(exact, high) : b.CreateICmpUGT(exact, high), high, exact);
  if (isSigned) {
    Value *low = ConstantInt::get(wide, APInt::getSignedMinValue(bits).sext(2 * bits));
    clamped = b.CreateSelect(b.CreateICmpSLT(clamped, low), low, clamped);
  }
  return b.CreateTrunc(clamped, type);
}

// Bit i of x moved to bit w - 1 - i, by definition.
Value *bitReverse(IRBuilder<> &b, Value *x) {
  auto *type = cast<IntegerType>(x->getType());
  unsigned bits = type->getBitWidth();
  Value *result = ConstantInt::get(type, 0);
  for (unsigned i = 0; i < bits; ++i) {
    Value *bit = b.CreateAnd(b.CreateLShr(x, i), 1);
    result = b.CreateOr(result, b.CreateShl(bit, bits - 1 - i));
  }
  return result;
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

void lowerIndirectBranches(Module &module) {
  LLVMContext &context = module.getContext();
  Type *word = Type::getInt64Ty(context);
  std::map<const BasicBlock *, uint64_t> numbers;
  std::vector<BlockAddress *> addresses;
  for (Function &function : module) {
    for (BasicBlock &block : function) {
      if (BlockAddress *address = BlockAddress::lookup(&block)) {
        numbers[&block] = numbers.size() + 1;
        addresses.push_back(address);
      }
    }
  }
  for (Function &function : module) {
    std::vector<IndirectBrInst *> branches;
    for (BasicBlock &block : function) {
      if (auto *branch = dyn_cast<IndirectBrInst>(block.getTerminator())) {
        branches.push_back(branch);
      }
    }
    BasicBlock *nowhere = nullptr;
    for (IndirectBrInst *branch : branches) {
      if (!nowhere) {
        nowhere = BasicBlock::Create(context, "indirectbr.nowhere", &function);
        new UnreachableInst(context, nowhere);
      }
      BasicBlock *block = branch->getParent();
      IRBuilder<> b(branch);
      Value *target = b.CreatePtrToInt(branch->getAddress(), word);
      SwitchInst *jump = b.CreateSwitch(target, nowhere, branch->getNumDestinations());
      std::set<BasicBlock *> seen;
      for (unsigned i = 0; i < branch->getNumDestinations(); ++i) {
        BasicBlock *destination = branch->getDestination(i);
        auto number = numbers.find(destination);
        if (!seen.insert(destination).second || number == numbers.end()) {
          // A second edge to the same block, or to a block whose address is never taken: unreachable.
          destination->removePredecessor(block, /*KeepOneInputPHIs=*/true);
          continue;
        }
        jump->addCase(cast<ConstantInt>(ConstantInt::get(word, number->second)), destination);
      }
      branch->eraseFromParent();
    }
  }
  for (BlockAddress *address : addresses) {
    auto number = numbers.at(address->getBasicBlock());
    address->replaceAllUsesWith(ConstantExpr::getIntToPtr(ConstantInt::get(word, number), address->getType()));
  }
}

namespace {

bool isBlank(StringRef text) { return text.trim().empty(); }

// The value an empty asm leaves in each of its outputs, and the stores it makes through indirect ones;
// false if some output would hold whatever its register held.
bool emptyAssembly(CallInst *call) {
  const auto *assembly = cast<InlineAsm>(call->getCalledOperand());
  if (!isBlank(assembly->getAsmString())) {
    return false;
  }
  InlineAsm::ConstraintInfoVector constraints = assembly->ParseConstraints();
  // The call operand of each constraint that has one.
  std::vector<int> operandOf(constraints.size(), -1);
  unsigned next = 0;
  for (unsigned i = 0; i < constraints.size(); ++i) {
    if (constraints[i].hasArg()) {
      operandOf[i] = int(next++);
    }
  }
  if (next != call->arg_size()) {
    return false;
  }
  std::vector<Value *> results;
  std::vector<std::pair<Value *, Value *>> stores; // (value, address)
  for (unsigned i = 0; i < constraints.size(); ++i) {
    const InlineAsm::ConstraintInfo &constraint = constraints[i];
    if (constraint.Type == InlineAsm::isLabel) {
      return false;
    }
    if (constraint.Type != InlineAsm::isOutput) {
      continue;
    }
    Value *tied = nullptr;
    if (constraint.hasMatchingInput()) {
      int input = operandOf[unsigned(constraint.MatchingInput)];
      if (input < 0) {
        return false;
      }
      tied = call->getArgOperand(unsigned(input));
    }
    if (!constraint.isIndirect) {
      if (!tied) {
        return false; // the output holds whatever was in its register
      }
      results.push_back(tied);
      continue;
    }
    Value *address = call->getArgOperand(unsigned(operandOf[i]));
    if (tied) {
      stores.push_back({tied, address});
      continue;
    }
    // Without a tied input, only a memory operand is left as it was.
    bool memoryOnly = !constraint.isMultipleAlternative && !constraint.Codes.empty();
    for (const std::string &code : constraint.Codes) {
      memoryOnly &= code == "m" || code == "o" || code == "V" || code == "Q";
    }
    if (!memoryOnly) {
      return false;
    }
  }
  Type *type = call->getType();
  if (results.size() != (type->isVoidTy() ? 0 : type->isStructTy() ? type->getStructNumElements() : 1)) {
    return false;
  }
  for (size_t i = 0; i < results.size(); ++i) {
    Type *expected = type->isStructTy() ? type->getStructElementType(unsigned(i)) : type;
    if (results[i]->getType() != expected) {
      return false;
    }
  }
  // Several outputs come back as a struct; each extractvalue of it reads one.
  if (type->isStructTy()) {
    for (User *user : call->users()) {
      auto *extract = dyn_cast<ExtractValueInst>(user);
      if (!extract || extract->getNumIndices() != 1) {
        return false;
      }
    }
  }
  IRBuilder<> b(call);
  for (auto [value, address] : stores) {
    b.CreateStore(value, address);
  }
  if (type->isStructTy()) {
    for (User *user : make_early_inc_range(call->users())) {
      auto *extract = cast<ExtractValueInst>(user);
      extract->replaceAllUsesWith(results[extract->getIndices()[0]]);
      extract->eraseFromParent();
    }
  } else if (!results.empty()) {
    call->replaceAllUsesWith(results[0]);
  }
  call->eraseFromParent();
  return true;
}

} // namespace

void lowerEmptyAssembly(Module &module) {
  for (Function &function : module) {
    std::vector<CallInst *> calls;
    for (Instruction &inst : instructions(function)) {
      if (auto *call = dyn_cast<CallInst>(&inst); call && call->isInlineAsm()) {
        calls.push_back(call);
      }
    }
    for (CallInst *call : calls) {
      emptyAssembly(call); // what is left is refused by the emitter
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
    std::vector<WeakVH> traps;
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
      case Intrinsic::trap:
      case Intrinsic::debugtrap:
      case Intrinsic::ubsantrap:
        traps.push_back(call); // after the others: this deletes the rest of the block
        continue;
      case Intrinsic::prefetch:
      case Intrinsic::expect:
      case Intrinsic::invariant_start:
      case Intrinsic::invariant_end:
        locate(function, call->getDebugLoc(), [&] { generic.LowerIntrinsicCall(call); });
        continue;
      case Intrinsic::sadd_with_overflow:
      case Intrinsic::uadd_with_overflow:
      case Intrinsic::ssub_with_overflow:
      case Intrinsic::usub_with_overflow:
      case Intrinsic::smul_with_overflow:
      case Intrinsic::umul_with_overflow: {
        if (!call->getArgOperand(0)->getType()->isIntegerTy()) {
          continue;
        }
        Intrinsic::ID id = call->getIntrinsicID();
        bool isSigned = id == Intrinsic::sadd_with_overflow || id == Intrinsic::ssub_with_overflow ||
                        id == Intrinsic::smul_with_overflow;
        Instruction::BinaryOps op =
            id == Intrinsic::sadd_with_overflow || id == Intrinsic::uadd_with_overflow   ? Instruction::Add
            : id == Intrinsic::ssub_with_overflow || id == Intrinsic::usub_with_overflow ? Instruction::Sub
                                                                                         : Instruction::Mul;
        locate(function, call->getDebugLoc(), [&] { withOverflow(call, op, isSigned); });
        continue;
      }
      case Intrinsic::sadd_sat:
      case Intrinsic::uadd_sat:
      case Intrinsic::ssub_sat:
      case Intrinsic::usub_sat: {
        if (!call->getType()->isIntegerTy()) {
          continue;
        }
        Intrinsic::ID id = call->getIntrinsicID();
        bool isSigned = id == Intrinsic::sadd_sat || id == Intrinsic::ssub_sat;
        bool add = id == Intrinsic::sadd_sat || id == Intrinsic::uadd_sat;
        replacement = saturated(b, call, add ? Instruction::Add : Instruction::Sub, isSigned);
        break;
      }
      case Intrinsic::bitreverse:
        if (!call->getType()->isIntegerTy()) {
          continue;
        }
        replacement = bitReverse(b, call->getArgOperand(0));
        break;
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
      case Intrinsic::ptrmask: {
        // The address with the mask applied: pointers are 64-bit integers, so all of it is address.
        Value *address = b.CreatePtrToInt(call->getArgOperand(0), call->getArgOperand(1)->getType());
        replacement = b.CreateIntToPtr(b.CreateAnd(address, call->getArgOperand(1)), call->getType());
        break;
      }
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
    for (WeakVH &trap : traps) {
      // The program stops here: the named fault `unreachable`. An earlier trap may have deleted it.
      if (trap) {
        changeToUnreachable(cast<Instruction>(trap));
      }
    }
  }
}

} // namespace bennett
