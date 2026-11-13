#include "Outline.h"

#include "Diagnostic.h"
#include "Markers.h"
#include "Taint.h"

#include <llvm/Analysis/DominanceFrontier.h>
#include <llvm/Analysis/PostDominators.h>
#include <llvm/Analysis/RegionInfo.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>
#include <llvm/Transforms/Utils/BasicBlockUtils.h>
#include <llvm/Transforms/Utils/Cloning.h>
#include <llvm/Transforms/Utils/CodeExtractor.h>
#include <llvm/Transforms/Utils/PromoteMemToReg.h>
#include <llvm/Transforms/Utils/ValueMapper.h>

#include <vector>

using namespace llvm;

namespace bennett {

std::set<const Function *> controlFunctions(const Module &module) {
  std::set<const Function *> control;
  bool changed = true;
  while (changed) {
    changed = false;
    for (const Function &function : module) {
      if (function.isDeclaration() || control.count(&function)) {
        continue;
      }
      for (const Instruction &inst : instructions(function)) {
        const auto *call = dyn_cast<CallBase>(&inst);
        if (call && (markerCall(call) || control.count(call->getCalledFunction()))) {
          control.insert(&function);
          changed = true;
          break;
        }
      }
    }
  }
  for (const Function *function : control) {
    for (const User *user : function->users()) {
      const auto *call = dyn_cast<CallBase>(user);
      if (!call || call->getCalledOperand() != function) {
        notHandled("the address of @" + function->getName().str() + ", which performs quantum operations, is taken",
                   user);
      }
    }
  }
  return control;
}

namespace {

// What a memory-writing instruction is, for messages.
std::string memoryWrite(const Instruction &inst) {
  if (isa<StoreInst>(inst)) {
    return "a store to memory";
  }
  if (isa<LoadInst>(inst)) {
    return "a volatile or atomic load";
  }
  if (const auto *call = dyn_cast<CallBase>(&inst)) {
    const Function *callee = call->getCalledFunction();
    return callee ? "a call to @" + callee->getName().str() + ", which may write memory,"
                  : "an indirect call that may write memory";
  }
  return "the instruction `" + std::string(inst.getOpcodeName()) + "`, which writes memory,";
}

void removeTrivialPhis(Function &function) {
  bool changed = true;
  while (changed) {
    changed = false;
    for (BasicBlock &block : function) {
      for (PHINode &phi : make_early_inc_range(block.phis())) {
        if (Value *value = phi.hasConstantValue(); value && value != &phi) {
          phi.replaceAllUsesWith(value);
          phi.eraseFromParent();
          changed = true;
        }
      }
    }
  }
}

// A copy of the extracted `region` that returns output `k` instead of storing all outputs through pointers.
Function *outputVariant(Function &region, unsigned inputs, const SetVector<Value *> &outputs, unsigned k) {
  Module &module = *region.getParent();
  LLVMContext &context = module.getContext();
  std::vector<Type *> params;
  for (unsigned i = 0; i < inputs; ++i) {
    params.push_back(region.getArg(i)->getType());
  }
  Type *result = outputs[k]->getType();
  auto *variant = Function::Create(FunctionType::get(result, params, false), GlobalValue::InternalLinkage,
                                   region.getName() + "." + Twine(k), &module);
  BasicBlock *entry = BasicBlock::Create(context, "outputs", variant);
  ValueToValueMapTy map;
  for (unsigned i = 0; i < inputs; ++i) {
    variant->getArg(i)->setName(region.getArg(i)->getName());
    map[region.getArg(i)] = variant->getArg(i);
  }
  std::vector<AllocaInst *> cells;
  for (unsigned j = 0; j < outputs.size(); ++j) {
    auto *cell = new AllocaInst(outputs[j]->getType(), module.getDataLayout().getAllocaAddrSpace(), "output", entry);
    cells.push_back(cell);
    map[region.getArg(inputs + j)] = cell;
  }
  SmallVector<ReturnInst *, 4> returns;
  CloneFunctionInto(variant, &region, map, CloneFunctionChangeType::GlobalChanges, returns);
  BranchInst::Create(cast<BasicBlock>(map[&region.getEntryBlock()]), entry);
  for (ReturnInst *ret : returns) {
    IRBuilder<> b(ret);
    b.CreateRet(b.CreateLoad(result, cells[k]));
    ret->eraseFromParent();
  }
  DominatorTree dominators(*variant);
  PromoteMemToReg(cells, dominators);
  return variant;
}

// Replaces the call of the extracted `region` by one call per used output.
void splitOutputs(Function &region, const SetVector<Value *> &inputs, const SetVector<Value *> &outputs) {
  if (!region.hasOneUse() || !isa<CallInst>(region.user_back())) {
    fail("CodeExtractor produced a region with other than one call");
  }
  auto *call = cast<CallInst>(region.user_back());
  if (!region.getReturnType()->isVoidTy() || region.arg_size() != inputs.size() + outputs.size()) {
    fail("CodeExtractor produced a region with more than one exit or aggregate arguments");
  }
  unsigned count = inputs.size();
  // Each output gets a copy of the region, and a region without used outputs disappears: that is only
  // right because outlineAround refused every write to memory in the region.
  for (Instruction &inst : instructions(region)) {
    auto *store = dyn_cast<StoreInst>(&inst);
    bool output = store && isa<Argument>(store->getPointerOperand()) &&
                  cast<Argument>(store->getPointerOperand())->getArgNo() >= count;
    if (inst.mayWriteToMemory() && !output) {
      fail("internal: an outlined region writes memory, at " + locationOf(&inst));
    }
  }
  std::vector<Value *> args(call->arg_begin(), call->arg_begin() + count);
  IRBuilder<> b(call);
  std::vector<Value *> cells;
  for (unsigned k = 0; k < outputs.size(); ++k) {
    Value *cell = call->getArgOperand(count + k);
    cells.push_back(cell);
    std::vector<LoadInst *> reloads;
    for (User *user : cell->users()) {
      if (auto *load = dyn_cast<LoadInst>(user)) {
        reloads.push_back(load);
      }
    }
    if (reloads.empty()) {
      continue;
    }
    Function *variant = outputVariant(region, count, outputs, k);
    CallInst *value = b.CreateCall(variant, args, outputs[k]->getName());
    for (LoadInst *reload : reloads) {
      reload->replaceAllUsesWith(value);
      reload->eraseFromParent();
    }
  }
  call->eraseFromParent();
  for (Value *cell : cells) {
    for (User *user : make_early_inc_range(cell->users())) {
      cast<Instruction>(user)->eraseFromParent(); // lifetime markers
    }
    if (auto *alloca = dyn_cast<AllocaInst>(cell)) {
      alloca->eraseFromParent();
    }
  }
  region.eraseFromParent();
}

void outlineAround(Function &function, Instruction *seed, ControlAnalysis &analysis,
                   const std::set<const Function *> &control) {
  BasicBlock *block = seed->getParent();
  bool branch = seed->isTerminator();
  DominatorTree dominators(function);
  PostDominatorTree postDominators(function);
  DominanceFrontier frontier;
  frontier.analyze(dominators);
  RegionInfo regions;
  regions.recalculate(function, &dominators, &postDominators, &frontier);

  // The smallest region that contains every edge the seed decides between: for a branch, any region
  // containing it; for a phi, one that contains all its incoming edges, or all but the single edge
  // that enters the region (a loop header).
  auto contains = [&](Region *region) {
    if (branch || region->getEntry() != block) {
      return true;
    }
    unsigned outside = 0;
    for (BasicBlock *pred : predecessors(block)) {
      outside += region->contains(pred) ? 0 : 1;
    }
    return outside == 1;
  };
  Region *region = regions.getRegionFor(block);
  while (!region->isTopLevelRegion() && !contains(region)) {
    region = region->getParent();
  }
  if (region->isTopLevelRegion() || region->getEntry() == &function.getEntryBlock()) {
    notHandled(branch ? "a branch on a value that depends on a qubit, whose arms do not rejoin before the function "
                        "returns (measure the value first)"
                      : "a value that depends on a qubit, merged by control flow that does not form a region",
               seed);
  }

  std::vector<BasicBlock *> blocks{region->getEntry()};
  for (BasicBlock *member : region->blocks()) {
    if (member != region->getEntry()) {
      blocks.push_back(member);
    }
  }
  std::vector<LoadInst *> loads;
  for (BasicBlock *member : blocks) {
    for (Instruction &inst : *member) {
      if (markerCall(&inst)) {
        notHandled(branch ? "a quantum operation controlled by a value that depends on a qubit"
                          : "a quantum operation inside control flow that carries a value that depends on a qubit",
                   &inst);
      }
      if (auto *store = dyn_cast<StoreInst>(&inst); store && analysis.access(store->getPointerOperand())) {
        notHandled("a store into qubit variable " + analysis.access(store->getPointerOperand())->storage->name +
                       " controlled by a value that depends on a qubit",
                   &inst);
      }
      if (auto *call = dyn_cast<CallBase>(&inst); call && control.count(call->getCalledFunction())) {
        notHandled("a call to @" + call->getCalledFunction()->getName().str() +
                       ", which performs quantum operations, controlled by a value that depends on a qubit",
                   &inst);
      }
      // The region becomes an ordinary function applied coherently, whose memory is private to each run:
      // a write to memory that outlives it would be lost. An ordinary write under quantum control acts
      // irreversibly on a quantum system, which is a channel, not a unitary.
      if (inst.mayWriteToMemory()) {
        notHandled(memoryWrite(inst) + " controlled by a value that depends on a qubit (measure the value first)",
                   &inst);
      }
      if (auto *load = dyn_cast<LoadInst>(&inst); load && analysis.access(load->getPointerOperand())) {
        loads.push_back(load);
      }
    }
  }

  // Nothing in the region changes a qubit variable, so its loads read what they would read just before
  // the region: hoist them there, and the region receives the values as arguments.
  if (!loads.empty()) {
    SmallVector<BasicBlock *, 4> outside;
    for (BasicBlock *pred : predecessors(region->getEntry())) {
      if (!region->contains(pred)) {
        outside.push_back(pred);
      }
    }
    BasicBlock *before = SplitBlockPredecessors(region->getEntry(), outside, ".qubits", &dominators);
    if (!before) {
      fail("cannot make a block before an outlined region");
    }
    IRBuilder<> b(before->getTerminator());
    for (LoadInst *load : loads) {
      Access source = *analysis.access(load->getPointerOperand());
      Value *pointer = b.CreateConstInBoundsGEP1_64(b.getInt8Ty(), source.storage->alloca, source.offset);
      LoadInst *hoisted = b.CreateLoad(load->getType(), pointer, load->getName());
      load->replaceAllUsesWith(hoisted);
      load->eraseFromParent();
    }
  }

  CodeExtractor extractor(blocks, &dominators, /*AggregateArgs=*/false, nullptr, nullptr, nullptr,
                          /*AllowVarArgs=*/false, /*AllowAlloca=*/true, nullptr, "region");
  if (!extractor.isEligible()) {
    notHandled("a region that depends on a qubit which LLVM's CodeExtractor cannot outline", block);
  }
  CodeExtractorAnalysisCache cache(function);
  SetVector<Value *> inputs, outputs;
  Function *outlined = extractor.extractCodeRegion(cache, inputs, outputs);
  if (!outlined) {
    notHandled("a region that depends on a qubit which LLVM's CodeExtractor failed to outline", block);
  }
  splitOutputs(*outlined, inputs, outputs);
}

// Attaches to every load of a qubit variable the range its support allows, and iterates: the support of
// a variable is the bits its gates act on and the bits any value stored into it may have, the latter
// computed by LLVM's known-bits analysis, which reads these ranges.
void annotateRanges(Function &function, ControlAnalysis &analysis) {
  LLVMContext &context = function.getContext();
  for (bool changed = true, first = true; changed; first = false) {
    analysis.computeSupport(first);
    changed = false;
    for (Instruction &inst : instructions(function)) {
      auto *load = dyn_cast<LoadInst>(&inst);
      std::optional<Access> source = load ? analysis.access(load->getPointerOperand()) : std::nullopt;
      if (!source) {
        continue;
      }
      unsigned width = load->getType()->getIntegerBitWidth();
      uint64_t mask = source->storage->support >> (8 * source->offset);
      if (width < 64) {
        mask &= (uint64_t(1) << width) - 1;
      }
      unsigned bits = 64 - countl_zero(mask);
      MDNode *range = nullptr;
      if (bits < width) {
        auto *type = cast<IntegerType>(load->getType());
        Metadata *bounds[] = {ConstantAsMetadata::get(ConstantInt::get(type, 0)),
                              ConstantAsMetadata::get(ConstantInt::get(type, uint64_t(1) << bits))};
        range = MDNode::get(context, bounds);
      }
      if (load->getMetadata(LLVMContext::MD_range) != range) {
        load->setMetadata(LLVMContext::MD_range, range);
        changed = true;
      }
      changed = changed || first;
    }
  }
}

} // namespace

void outlineRegions(Function &function, bool taintAll, const std::set<const Function *> &control) {
  {
    // Lifetime markers on qubit variables mean nothing (a qubit variable lives as long as its function)
    // and would make outlined regions take their addresses.
    ControlAnalysis analysis(function, taintAll);
    for (const auto &storage : analysis.storages) {
      for (User *user : make_early_inc_range(storage->alloca->users())) {
        if (auto *intrinsic = dyn_cast<IntrinsicInst>(user); intrinsic && intrinsic->isLifetimeStartOrEnd()) {
          intrinsic->eraseFromParent();
        }
      }
    }
  }
  for (;;) {
    removeTrivialPhis(function);
    ControlAnalysis analysis(function, taintAll);
    Instruction *seed = analysis.seed();
    if (!seed) {
      annotateRanges(function, analysis);
      return;
    }
    // A tainted branch goes into the region alone, so the code before it stays in the control program.
    if (seed->isTerminator() && seed->getParent()->getFirstNonPHI() != seed) {
      SplitBlock(seed->getParent(), seed);
      continue;
    }
    outlineAround(function, seed, analysis, control);
  }
}

} // namespace bennett
