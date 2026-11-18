#include "Outline.h"

#include "CallerPosition.h"
#include "Diagnostic.h"
#include "HeldBytes.h"
#include "InPlace.h"
#include "Markers.h"
#include "MovedLoop.h"
#include "Predicate.h"
#include "RegionResults.h"
#include "Taint.h"

#include <llvm/Analysis/DominanceFrontier.h>
#include <llvm/Analysis/Loads.h>
#include <llvm/Analysis/PostDominators.h>
#include <llvm/Analysis/RegionInfo.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>
#include <llvm/Transforms/Utils/BasicBlockUtils.h>
#include <llvm/Transforms/Utils/Cloning.h>
#include <llvm/Transforms/Utils/CodeExtractor.h>
#include <llvm/Transforms/Utils/Local.h>
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
  bool emittedOutput = false;
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
    if (emittedOutput) {
      requireCallerIndependent(region, *call, " when splitting the outputs of a read-only region");
    }
    emittedOutput = true;
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
  // These effects would be rejected before predication. For a gate loop, give the
  // route for moving the ordinary work into its applied table computation.
  auto inGateLoop = [&](const BasicBlock *block) {
    const Loop *loop = analysis.loops.getLoopFor(block);
    if (!loop || !region->contains(loop->getHeader())) {
      return false;
    }
    while (loop->getParentLoop() && region->contains(loop->getParentLoop()->getHeader())) {
      loop = loop->getParentLoop();
    }
    return llvm::any_of(loop->blocks(), [](const BasicBlock *member) {
      return llvm::any_of(*member, [](const Instruction &inst) { return isGateCall(&inst); });
    });
  };
  std::vector<LoadInst *> loads;
  std::vector<LoadInst *> memoryLoads; // loads of ordinary memory at an address fixed before the region
  std::vector<Instruction *> memoryWrites; // stores into memory: its bytes will hold qubits (HeldBytes.h)
  bool variablePosition = false;          // a load from a qubit variable at a position that is not constant
  for (BasicBlock *member : blocks) {
    for (Instruction &inst : *member) {
      if (std::optional<Marker> marker = markerCall(&inst);
          marker && (marker->kind == MarkerKind::Measure || marker->kind == MarkerKind::MeasureBytes)) {
        notHandled("a measurement under the control of a value that depends on a qubit", &inst);
      }
      if (markerCall(&inst)) {
        continue; // gates and the qubits they act on: predicated below
      }
      if (auto *store = dyn_cast<StoreInst>(&inst); store && analysis.basedOn(store->getPointerOperand())) {
        continue; // predicated below
      }
      if (auto *call = dyn_cast<CallBase>(&inst); call && control.count(call->getCalledFunction())) {
        notHandled("a call to @" + call->getCalledFunction()->getName().str() +
                       ", which performs quantum operations, controlled by a value that depends on a qubit",
                   &inst);
      }
      // The region becomes an ordinary function applied coherently, whose memory is private to each run: a
      // store into memory that outlives it makes the bytes it writes hold qubits (HeldBytes.h). A call's
      // writes would be undone with it, unlike natively.
      if (inst.mayWriteToMemory() && inGateLoop(member)) {
        requireSafeMovedLoopInstruction(inst);
      }
      if (auto *store = dyn_cast<StoreInst>(&inst); store && store->isSimple()) {
        memoryWrites.push_back(store);
        continue;
      }
      if (inst.mayWriteToMemory()) {
        notHandled(memoryWrite(inst) +
                       " controlled by a value that depends on a qubit (not built: " +
                       (isa<CallBase>(inst) ? "a call that fills memory named at the call, HANDOFF.md)" : "HANDOFF.md)"),
                   &inst);
      }
      if (auto *load = dyn_cast<LoadInst>(&inst); load && analysis.access(load->getPointerOperand())) {
        loads.push_back(load);
      } else if (load && analysis.basedOn(load->getPointerOperand())) {
        variablePosition = true;
      } else if (load && load->isSimple()) {
        const auto *pointer = dyn_cast<Instruction>(load->getPointerOperand());
        if (!pointer || !region->contains(pointer->getParent())) {
          memoryLoads.push_back(load);
        }
      }
    }
  }

  // Stores into memory: the bytes they write hold qubits from a point before the region on, kept in a qubit
  // variable while they do (HeldBytes.h). Then the region is looked at again.
  if (!memoryWrites.empty()) {
    if (hasGates(blocks)) {
      notHandled("a store to memory under a value that depends on a qubit, in code that also has gates (not built: "
                 "write the memory in code without gates, HANDOFF.md)",
                 memoryWrites.front());
    }
    SmallVector<BasicBlock *, 4> outside;
    for (BasicBlock *pred : predecessors(region->getEntry())) {
      if (!region->contains(pred)) {
        outside.push_back(pred);
      }
    }
    BasicBlock *before = SplitBlockPredecessors(region->getEntry(), outside, ".held", &dominators);
    if (!before) {
      fail("cannot make a block before a region that writes memory");
    }
    holdWrites(function, memoryWrites, before->getTerminator(), {blocks.begin(), blocks.end()}, analysis);
    return;
  }

  // Without gates, the region is one applied function for all the qubit variables it writes; otherwise (or
  // when a variable it writes is neither known before it nor only XORed into, and it has no loop) its gates
  // and stores move after it, each acting where its path reaches it.
  if (hasQuantumOperations(blocks, analysis)) {
    if (!hasGates(blocks) && applyRegionWhole(function, blocks, region->getExit(), analysis)) {
      return;
    }
    for (BasicBlock *member : blocks) {
      for (Instruction &inst : *member) {
        auto *store = dyn_cast<StoreInst>(&inst);
        const Storage *storage = store ? analysis.basedOn(store->getPointerOperand()) : nullptr;
        std::optional<Access> place = storage ? analysis.access(store->getPointerOperand()) : std::nullopt;
        if (storage && (!place || place->offset != 0 ||
                        function.getParent()->getDataLayout().getTypeStoreSizeInBits(
                            store->getValueOperand()->getType()) != storage->bits)) {
          notHandled("a store into part of qubit variable " + storage->name +
                         " under a value that depends on a qubit, in code with gates or after its value became "
                         "unknown (not built, HANDOFF.md)",
                     store);
        }
      }
    }
    predicateRegion(function, blocks, region->getExit(), analysis, control);
    return;
  }
  if (variablePosition) {
    notHandled("a read at a position that is not constant of a qubit variable or of memory that holds qubits, "
               "in code under a qubit that writes no qubit variable (not built, HANDOFF.md)",
               block);
  }

  // Nothing in the region changes a qubit variable or writes memory, so its loads read what they would read just
  // before the region: hoist them there, and the region receives the values as arguments. The control program
  // computes the region's value where it is used (Control.cpp), which may be after memory the region read has
  // changed (the span of bytes written there may start in between, HeldBytes.h). Read a fixed address where the
  // region is only if LLVM says it is safe on every path and the front knows the bytes are ordinary there.
  const DataLayout &layout = function.getParent()->getDataLayout();
  Instruction *top = &*region->getEntry()->getFirstInsertionPt();
  erase_if(memoryLoads, [&](LoadInst *load) {
    return !isSafeToLoadUnconditionally(load->getPointerOperand(), load->getType(), load->getAlign(), layout, top,
                                        nullptr, &dominators);
  });
  keepOrdinaryLoads(memoryLoads, *top, control);
  if (!loads.empty() || !memoryLoads.empty()) {
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
    for (LoadInst *load : memoryLoads) {
      load->moveBefore(before->getTerminator());
    }
  }

  // Dead code left by earlier steps (an address into a qubit variable whose store has moved out) would become
  // an input of the outlined function.
  for (bool changed = true; changed;) {
    changed = false;
    for (BasicBlock *member : blocks) {
      for (Instruction &inst : make_early_inc_range(reverse(*member))) {
        if (isInstructionTriviallyDead(&inst)) {
          inst.eraseFromParent();
          changed = true;
        }
      }
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
    if (seed && holdStraightWrite(function, analysis, seed)) {
      continue; // its bytes are held before code under a qubit writes them too
    }
    if (!seed) {
      if (holdStraightWrite(function, analysis) || finishHeld(function, analysis)) {
        continue;
      }
      if (rewriteInPlaceUpdate(function, analysis)) {
        continue;
      }
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
