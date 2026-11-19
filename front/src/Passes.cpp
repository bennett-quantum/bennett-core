#include "Passes.h"

#include "Classify.h"
#include "Diagnostic.h"

#include <llvm/ADT/SmallPtrSet.h>
#include <llvm/Analysis/AliasAnalysis.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/Module.h>
#include <llvm/Passes/PassBuilder.h>
#include <llvm/Support/CrashRecoveryContext.h>
#include <llvm/Transforms/InstCombine/InstCombine.h>
#include <llvm/Transforms/Scalar/DCE.h>
#include <llvm/Transforms/Scalar/EarlyCSE.h>
#include <llvm/Transforms/Scalar/Scalarizer.h>
#include <llvm/Transforms/Utils/ModuleUtils.h>

#include <string>
#include <vector>

using namespace llvm;

namespace bennett {

namespace {

// Fresh analysis managers for one run, with LLVM's default alias analysis.
struct Analyses {
  LoopAnalysisManager loops;
  FunctionAnalysisManager functions;
  CGSCCAnalysisManager sccs;
  ModuleAnalysisManager modules;
  PassBuilder builder;

  explicit Analyses(PipelineTuningOptions tuning = PipelineTuningOptions()) : builder(nullptr, tuning) {
    functions.registerPass([this] { return builder.buildDefaultAAPipeline(); });
    builder.registerModuleAnalyses(modules);
    builder.registerCGSCCAnalyses(sccs);
    builder.registerFunctionAnalyses(functions);
    builder.registerLoopAnalyses(loops);
    builder.crossRegisterProxies(loops, functions, sccs, modules);
  }
  Analyses(const Analyses &) = delete;
  Analyses &operator=(const Analyses &) = delete;

  ModulePassManager parse(const char *pipeline) {
    ModulePassManager passes;
    if (Error error = builder.parsePassPipeline(passes, pipeline)) {
      fail("internal: " + toString(std::move(error)));
    }
    return passes;
  }
};

// What a sealed instruction was, to make it again.
struct Original {
  enum Kind { Load, Store, Update, Exchange, Call };
  Kind kind = Load;
  Type *type = nullptr; // the type a load reads, an atomic update or exchange works on
  Align align;
  bool isVolatile = false;
  bool weak = false; // cmpxchg
  AtomicOrdering ordering = AtomicOrdering::NotAtomic;
  AtomicOrdering failure = AtomicOrdering::NotAtomic; // cmpxchg
  AtomicRMWInst::BinOp operation = AtomicRMWInst::BAD_BINOP;
  SyncScope::ID scope = SyncScope::System;
  // A call: its function type, its callee (null for a call through a pointer, which the sealed call passes
  // first), or the name of an intrinsic, whose declaration a pass may delete while nothing calls it.
  FunctionType *calleeType = nullptr;
  Function *callee = nullptr;
  std::string intrinsic;
  AttributeList attributes;
  CallingConv::ID convention = CallingConv::C;
  CallInst::TailCallKind tail = CallInst::TCK_None;
  SmallVector<std::pair<unsigned, MDNode *>, 4> metadata; // bennett.class among them
  std::string what;        // the class's short name
  std::string description; // the class, for messages
  std::string where; // the source location, for messages
  std::string function;
};

class Seal {
public:
  // Seals the classified instructions of `functions`.
  Seal(Module &module, const std::vector<Function *> &functions);
  Seal(const Seal &) = delete;
  Seal &operator=(const Seal &) = delete;
  // Makes every sealed call the instruction it stood for again, and refuses by name an instruction that `run`
  // removed or copied.
  void open(const char *run);

private:
  Module &module;
  // Each declaration and the instruction its one call stands for, in the order they were sealed.
  std::vector<std::pair<Function *, Original>> sealed;
  std::vector<GlobalValue *> kept;       // callees kept in llvm.compiler.used while their calls are sealed

  void seal(Instruction &inst);
  Instruction *restore(CallInst &call, const Original &original);
};

unsigned sealedCount = 0; // makes the declarations' names unique within the run of the front

Seal::Seal(Module &module, const std::vector<Function *> &functions) : module(module) {
  std::vector<Instruction *> marked;
  for (Function *function : functions) {
    for (Instruction &inst : instructions(*function)) {
      if (classOf(inst)) {
        marked.push_back(&inst);
      }
    }
  }
  for (Instruction *inst : marked) {
    seal(*inst);
  }
  SmallPtrSet<Constant *, 8> already;
  SmallVector<GlobalValue *, 8> present;
  collectUsedGlobalVariables(module, present, /*CompilerUsed=*/true);
  already.insert(present.begin(), present.end());
  std::vector<GlobalValue *> fresh;
  for (GlobalValue *value : kept) {
    if (already.insert(value).second) {
      fresh.push_back(value);
    }
  }
  kept = fresh;
  if (!kept.empty()) {
    appendToCompilerUsed(module, kept);
  }
}

void Seal::seal(Instruction &inst) {
  Original original;
  inst.getAllMetadata(original.metadata);
  original.what = className(*classOf(inst));
  original.description = classDescription(*classOf(inst));
  original.where = locationOf(&inst);
  original.function = inst.getFunction()->getName().str();
  Type *result = inst.getType();
  std::vector<Value *> operands;
  if (auto *load = dyn_cast<LoadInst>(&inst)) {
    original.kind = Original::Load;
    original.type = load->getType();
    original.align = load->getAlign();
    original.isVolatile = load->isVolatile();
    original.ordering = load->getOrdering();
    original.scope = load->getSyncScopeID();
    operands = {load->getPointerOperand()};
  } else if (auto *store = dyn_cast<StoreInst>(&inst)) {
    original.kind = Original::Store;
    original.align = store->getAlign();
    original.isVolatile = store->isVolatile();
    original.ordering = store->getOrdering();
    original.scope = store->getSyncScopeID();
    operands = {store->getValueOperand(), store->getPointerOperand()};
  } else if (auto *update = dyn_cast<AtomicRMWInst>(&inst)) {
    original.kind = Original::Update;
    original.type = update->getType();
    original.align = update->getAlign();
    original.isVolatile = update->isVolatile();
    original.ordering = update->getOrdering();
    original.operation = update->getOperation();
    original.scope = update->getSyncScopeID();
    operands = {update->getPointerOperand(), update->getValOperand()};
  } else if (auto *exchange = dyn_cast<AtomicCmpXchgInst>(&inst)) {
    original.kind = Original::Exchange;
    original.align = exchange->getAlign();
    original.isVolatile = exchange->isVolatile();
    original.weak = exchange->isWeak();
    original.ordering = exchange->getSuccessOrdering();
    original.failure = exchange->getFailureOrdering();
    original.scope = exchange->getSyncScopeID();
    operands = {exchange->getPointerOperand(), exchange->getCompareOperand(), exchange->getNewValOperand()};
  } else if (auto *call = dyn_cast<CallInst>(&inst)) {
    original.kind = Original::Call;
    if (call->hasOperandBundles()) {
      fail("internal: a classified call with operand bundles, at " + original.where);
    }
    original.calleeType = call->getFunctionType();
    original.attributes = call->getAttributes();
    original.convention = call->getCallingConv();
    original.tail = call->getTailCallKind();
    Function *callee = call->getCalledFunction();
    if (!callee) {
      operands.push_back(call->getCalledOperand());
    } else if (callee->isIntrinsic()) {
      original.intrinsic = callee->getName().str();
    } else {
      original.callee = callee;
      kept.push_back(callee);
    }
    operands.insert(operands.end(), call->arg_begin(), call->arg_end());
  } else {
    fail("internal: the instruction `" + std::string(inst.getOpcodeName()) + "` carries a class, at " +
         original.where);
  }
  std::vector<Type *> types;
  for (Value *operand : operands) {
    types.push_back(operand->getType());
  }
  std::string name = "bennett.sealed." + original.what + "." + std::to_string(sealedCount++);
  Function *declaration =
      Function::Create(FunctionType::get(result, types, false), GlobalValue::ExternalLinkage, name, module);
  declaration->addFnAttr(Attribute::NoDuplicate); // a pass may not copy the call (Passes.h)
  CallInst *call = CallInst::Create(declaration, operands, "", &inst);
  call->setDebugLoc(inst.getDebugLoc());
  if (!result->isVoidTy()) {
    call->takeName(&inst);
    inst.replaceAllUsesWith(call);
  }
  inst.eraseFromParent();
  sealed.emplace_back(declaration, std::move(original));
}

Instruction *Seal::restore(CallInst &call, const Original &original) {
  Instruction *back = nullptr;
  auto arg = [&](unsigned i) { return call.getArgOperand(i); };
  switch (original.kind) {
  case Original::Load:
    back = new LoadInst(original.type, arg(0), "", original.isVolatile, original.align, original.ordering,
                        original.scope, &call);
    break;
  case Original::Store:
    back = new StoreInst(arg(0), arg(1), original.isVolatile, original.align, original.ordering, original.scope,
                         &call);
    break;
  case Original::Update: {
    auto *update =
        new AtomicRMWInst(original.operation, arg(0), arg(1), original.align, original.ordering, original.scope, &call);
    update->setVolatile(original.isVolatile);
    back = update;
    break;
  }
  case Original::Exchange: {
    auto *exchange = new AtomicCmpXchgInst(arg(0), arg(1), arg(2), original.align, original.ordering,
                                           original.failure, original.scope, &call);
    exchange->setVolatile(original.isVolatile);
    exchange->setWeak(original.weak);
    back = exchange;
    break;
  }
  case Original::Call: {
    std::vector<Value *> args(call.arg_begin(), call.arg_end());
    FunctionCallee callee;
    if (original.callee) {
      callee = FunctionCallee(original.calleeType, original.callee);
    } else if (!original.intrinsic.empty()) {
      callee = module.getOrInsertFunction(original.intrinsic, original.calleeType);
    } else {
      callee = FunctionCallee(original.calleeType, args.front());
      args.erase(args.begin());
    }
    auto *restored = CallInst::Create(callee, args, "", &call);
    restored->setCallingConv(original.convention);
    restored->setAttributes(original.attributes);
    restored->setTailCallKind(original.tail);
    back = restored;
    break;
  }
  }
  for (const auto &[kind, node] : original.metadata) {
    back->setMetadata(kind, node);
  }
  back->setDebugLoc(call.getDebugLoc());
  if (!call.getType()->isVoidTy()) {
    back->takeName(&call);
    call.replaceAllUsesWith(back);
  }
  return back;
}

void Seal::open(const char *run) {
  for (auto &[declaration, original] : sealed) {
    std::vector<CallInst *> calls;
    for (User *user : declaration->users()) {
      auto *call = dyn_cast<CallInst>(user);
      if (!call || call->getCalledOperand() != declaration) {
        fail(std::string("internal: ") + run + " used a sealed instruction other than by calling it, at " +
             original.where);
      }
      calls.push_back(call);
    }
    if (calls.size() != 1) {
      Function *function = module.getFunction(original.function);
      notHandled(std::string(run) + (calls.empty() ? " removed " : " copied ") + "an instruction whose class the " +
                     "front decided before it (" + original.description + ", at " + original.where +
                     "); the front's translation depends on that instruction standing exactly where C has it, once",
                 calls.empty() ? static_cast<const Value *>(function) : calls.front());
    }
    restore(*calls.front(), original);
    calls.front()->eraseFromParent();
    declaration->eraseFromParent();
  }
  sealed.clear();
  if (!kept.empty()) {
    // Only the entries this seal added to llvm.compiler.used go; llvm.used and the other entries stay as they were.
    SmallPtrSet<GlobalValue *, 8> remove(kept.begin(), kept.end());
    SmallVector<GlobalValue *, 8> rest;
    // The list may be gone: LLVM's GlobalOpt drops an entry of it that llvm.used also holds.
    if (GlobalVariable *list = collectUsedGlobalVariables(module, rest, /*CompilerUsed=*/true)) {
      erase_if(rest, [&](GlobalValue *value) { return remove.count(value) != 0; });
      list->eraseFromParent();
      if (!rest.empty()) {
        appendToCompilerUsed(module, rest);
      }
    }
    kept.clear();
  }
}

std::vector<Function *> definedFunctions(Module &module) {
  std::vector<Function *> functions;
  for (Function &function : module) {
    if (!function.isDeclaration()) {
      functions.push_back(&function);
    }
  }
  return functions;
}

} // namespace

void optimiseSealed(Module &module) {
  for (Function &function : module) {
    if (function.hasFnAttribute(Attribute::OptimizeNone)) {
      function.removeFnAttr(Attribute::OptimizeNone);
      function.removeFnAttr(Attribute::NoInline);
    }
  }
  Seal seal(module, definedFunctions(module));
  PipelineTuningOptions tuning;
  tuning.LoopVectorization = false; // the device program has no vector values
  tuning.SLPVectorization = false;
  // LLVM's optimiser has bugs of its own. A crash inside it is reported by name; no device program is written.
  CrashRecoveryContext::Enable();
  CrashRecoveryContext recovery;
  bool finished = recovery.RunSafely([&] {
    Analyses analyses(tuning);
    ModulePassManager passes = analyses.builder.buildPerModuleDefaultPipeline(OptimizationLevel::O2);
    passes.run(module, analyses.modules);
  });
  CrashRecoveryContext::Disable();
  if (!finished) {
    fail("LLVM's optimiser crashed on this module (a bug in LLVM); no device program was written");
  }
  seal.open("LLVM's optimiser");
  // Memory effects inferred while instructions were sealed are too weak.
  inferAttributes(module);
}

void promoteLocals(Module &module) {
  Seal seal(module, definedFunctions(module));
  Analyses analyses;
  ModulePassManager passes = analyses.parse("function(sroa<preserve-cfg>)");
  passes.run(module, analyses.modules);
  seal.open("LLVM's SROA");
}

void inferAttributes(Module &module) {
  Analyses analyses;
  ModulePassManager passes = analyses.parse("cgscc(function-attrs)");
  passes.run(module, analyses.modules);
}

void removeDeadGlobals(Module &module) {
  Analyses analyses;
  ModulePassManager passes = analyses.parse("globaldce,cgscc(function-attrs)");
  passes.run(module, analyses.modules);
}

void scalariseSealed(Module &module) {
  Seal seal(module, definedFunctions(module));
  Analyses analyses;
  ScalarizerPassOptions options;
  options.ScalarizeLoadStore = true;
  options.ScalarizeVariableInsertExtract = true;
  FunctionPassManager passes;
  passes.addPass(ScalarizerPass(options));
  passes.addPass(InstCombinePass());
  passes.addPass(DCEPass());
  ModulePassManager modulePasses;
  modulePasses.addPass(createModuleToFunctionPassAdaptor(std::move(passes)));
  modulePasses.run(module, analyses.modules);
  seal.open("LLVM's Scalarizer and InstCombine");
}

void simplify(Function &function) {
  Seal seal(*function.getParent(), {&function});
  Analyses analyses;
  FunctionPassManager passes;
  passes.addPass(InstCombinePass());
  passes.addPass(EarlyCSEPass());
  passes.addPass(InstCombinePass());
  passes.run(function, analyses.functions);
  seal.open("LLVM's simplifier");
}

void simplifyDeep(Function &function) {
  Seal seal(*function.getParent(), {&function});
  Analyses analyses;
  FunctionPassManager passes;
  if (Error error = analyses.builder.parsePassPipeline(
          passes, "sccp,instcombine,simplifycfg,early-cse,instcombine,simplifycfg,adce,instcombine,simplifycfg")) {
    fail("internal: " + toString(std::move(error)));
  }
  passes.run(function, analyses.functions);
  seal.open("LLVM's simplifier");
}

} // namespace bennett
