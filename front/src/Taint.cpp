#include "Taint.h"

#include "Diagnostic.h"
#include "Markers.h"

#include <llvm/ADT/PostOrderIterator.h>
#include <llvm/Analysis/CFG.h>
#include <llvm/Analysis/ValueTracking.h>
#include <llvm/IR/Constants.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/Module.h>

using namespace llvm;

namespace bennett {

namespace {

constexpr unsigned maxRegisterBytes = 8; // the device program's widest value

Fact meet(const Fact &a, const Fact &b) {
  if (a.kind == Fact::Unvisited) {
    return b;
  }
  if (b.kind == Fact::Unvisited) {
    return a;
  }
  if (a.kind == Fact::Known && b.kind == Fact::Known && a.value == b.value) {
    return a;
  }
  return Fact{Fact::Quantum, nullptr};
}

bool sameFacts(const std::vector<Fact> &a, const std::vector<Fact> &b) {
  if (a.size() != b.size()) {
    return false;
  }
  for (size_t i = 0; i < a.size(); ++i) {
    if (a[i].kind != b[i].kind || a[i].value != b[i].value) {
      return false;
    }
  }
  return true;
}

std::string variableName(const AllocaInst *alloca) {
  return alloca->hasName() ? "'" + alloca->getName().str() + "'" : "a local variable";
}

} // namespace

ControlAnalysis::ControlAnalysis(Function &function, bool taintAll)
    : dominators(function), loops(dominators), function(function), taintAll(taintAll) {
  ReversePostOrderTraversal<Function *> rpot(&function);
  order.assign(rpot.begin(), rpot.end());
  findStorage();
  for (auto &storage : storages) {
    checkUses(*storage);
  }
  // Known facts make loads classical, and stores of tainted values make variables Quantum: iterate to the
  // fixed point, in which taint only grows.
  do {
    runDataflow();
  } while (computeTaint());
  computeSupport();
}

void ControlAnalysis::computeSupport(bool gatesOnly) {
  const DataLayout &layout = function.getParent()->getDataLayout();
  for (auto &storage : storages) {
    uint64_t support = 0;
    for (Instruction *mutation : storage->mutations) {
      if (auto *store = dyn_cast<StoreInst>(mutation)) {
        if (gatesOnly) {
          continue;
        }
        support |= (~computeKnownBits(store->getValueOperand(), layout).Zero).getZExtValue();
      } else if (markerCall(mutation)->kind != MarkerKind::Measure) {
        support |= uint64_t(1) << (8 * access(cast<CallBase>(mutation)->getArgOperand(0))->offset);
      }
    }
    storage->support = storage->bits == 64 ? support : support & ((uint64_t(1) << storage->bits) - 1);
  }
}

void ControlAnalysis::findStorage() {
  const DataLayout &layout = function.getParent()->getDataLayout();
  for (Instruction &inst : instructions(function)) {
    if (!markerCall(&inst)) {
      continue;
    }
    auto *call = cast<CallBase>(&inst);
    APInt offset(64, 0);
    const Value *base = call->getArgOperand(0)->stripAndAccumulateConstantOffsets(layout, offset, true);
    const auto *alloca = dyn_cast<AllocaInst>(base);
    if (!alloca) {
      notHandled("a marker applied to something other than a local variable", call);
    }
    if (access(alloca)) {
      continue;
    }
    std::optional<TypeSize> size = alloca->getAllocationSize(layout);
    if (!alloca->isStaticAlloca() || !size || size->isScalable()) {
      notHandled("a qubit variable whose size is not fixed", alloca);
    }
    if (size->getFixedValue() > maxRegisterBytes || size->getFixedValue() == 0) {
      notHandled("qubit variable " + variableName(alloca) + " is wider than 64 bits", alloca);
    }
    auto storage = std::make_unique<Storage>();
    storage->alloca = const_cast<AllocaInst *>(alloca);
    storage->bits = unsigned(size->getFixedValue() * 8);
    storage->name = variableName(alloca);
    storages.push_back(std::move(storage));
  }
}

void ControlAnalysis::checkUses(Storage &storage) {
  const DataLayout &layout = function.getParent()->getDataLayout();
  const unsigned bytes = storage.bits / 8;
  std::vector<std::pair<Value *, uint64_t>> pointers{{storage.alloca, 0}};
  auto other = [&](const Instruction *user) {
    notHandled("the address of qubit variable " + storage.name +
                   " is used by ordinary code; only markers, loads and whole stores may use it",
               user);
  };
  while (!pointers.empty()) {
    auto [pointer, offset] = pointers.back();
    pointers.pop_back();
    for (User *user : pointer->users()) {
      auto *inst = dyn_cast<Instruction>(user);
      if (!inst) {
        notHandled("the address of qubit variable " + storage.name + " is used in a constant", storage.alloca);
      }
      if (auto *gep = dyn_cast<GetElementPtrInst>(inst)) {
        APInt delta(64, 0);
        if (gep->getPointerOperand() != pointer || !gep->accumulateConstantOffset(layout, delta)) {
          other(inst);
        }
        pointers.push_back({gep, offset + delta.getZExtValue()});
        continue;
      }
      if (auto *load = dyn_cast<LoadInst>(inst)) {
        uint64_t size = layout.getTypeStoreSize(load->getType()).getFixedValue();
        if (offset + size > bytes || !load->getType()->isIntegerTy()) {
          notHandled("a load outside qubit variable " + storage.name + " or not of an integer", inst);
        }
        continue;
      }
      if (auto *store = dyn_cast<StoreInst>(inst)) {
        uint64_t size = layout.getTypeStoreSize(store->getValueOperand()->getType()).getFixedValue();
        if (store->getValueOperand() == pointer) {
          other(inst);
        }
        if (offset != 0 || size != bytes || !store->getValueOperand()->getType()->isIntegerTy()) {
          notHandled("a store into part of qubit variable " + storage.name +
                         "; only a store of the whole variable is handled",
                     inst);
        }
        storage.mutations.push_back(store);
        continue;
      }
      if (std::optional<Marker> marker = markerCall(inst)) {
        auto *call = cast<CallBase>(inst);
        bool asQubit = call->getArgOperand(0) == pointer;
        for (unsigned i = 1; i < call->arg_size(); ++i) {
          asQubit = asQubit && call->getArgOperand(i) != pointer;
        }
        if (!asQubit || offset >= bytes) {
          other(inst);
        }
        if (marker->kind == MarkerKind::Measure && offset != 0) {
          notHandled("a measurement of part of qubit variable " + storage.name, inst);
        }
        storage.mutations.push_back(call);
        continue;
      }
      if (isa<CallBase>(inst)) {
        notHandled("qubit variable " + storage.name + " is passed to a function; only markers may take its address",
                   inst);
      }
      other(inst);
    }
  }
}

std::optional<Access> ControlAnalysis::access(const Value *pointer) const {
  if (!pointer->getType()->isPointerTy()) {
    return std::nullopt;
  }
  const DataLayout &layout = function.getParent()->getDataLayout();
  APInt offset(64, 0);
  const Value *base = pointer->stripAndAccumulateConstantOffsets(layout, offset, true);
  for (const auto &storage : storages) {
    if (storage->alloca == base) {
      return Access{storage.get(), unsigned(offset.getZExtValue())};
    }
  }
  return std::nullopt;
}

size_t ControlAnalysis::indexOf(const Storage *storage) const {
  for (size_t i = 0; i < storages.size(); ++i) {
    if (storages[i].get() == storage) {
      return i;
    }
  }
  return storages.size();
}

void ControlAnalysis::transfer(const Instruction &inst, std::vector<Fact> &facts) const {
  if (const auto *store = dyn_cast<StoreInst>(&inst)) {
    if (std::optional<Access> target = access(store->getPointerOperand())) {
      Value *value = const_cast<Value *>(store->getValueOperand());
      facts[indexOf(target->storage)] = tainted(value) ? Fact{Fact::Quantum, nullptr} : Fact{Fact::Known, value};
    }
    return;
  }
  if (std::optional<Marker> marker = markerCall(&inst)) {
    const auto *call = cast<CallBase>(&inst);
    if (std::optional<Access> target = access(call->getArgOperand(0))) {
      Fact &fact = facts[indexOf(target->storage)];
      fact = marker->kind == MarkerKind::Measure ? Fact{Fact::Known, const_cast<CallBase *>(call)}
                                                 : Fact{Fact::Quantum, nullptr};
    }
  }
}

void ControlAnalysis::runDataflow() {
  std::vector<Fact> initial;
  for (const auto &storage : storages) {
    initial.push_back(Fact{Fact::Known, ConstantInt::get(IntegerType::get(function.getContext(), storage->bits), 0)});
  }
  std::map<const BasicBlock *, std::vector<Fact>> exitFacts;
  entryFacts.clear();
  bool changed = true;
  while (changed) {
    changed = false;
    for (BasicBlock *block : order) {
      std::vector<Fact> facts(storages.size());
      if (block == &function.getEntryBlock()) {
        facts = initial;
      } else {
        for (BasicBlock *pred : predecessors(block)) {
          auto found = exitFacts.find(pred);
          if (found == exitFacts.end()) {
            continue;
          }
          for (size_t i = 0; i < facts.size(); ++i) {
            facts[i] = meet(facts[i], found->second[i]);
          }
        }
      }
      entryFacts[block] = facts;
      for (const Instruction &inst : *block) {
        transfer(inst, facts);
      }
      auto previous = exitFacts.find(block);
      if (previous == exitFacts.end() || !sameFacts(previous->second, facts)) {
        exitFacts[block] = facts;
        changed = true;
      }
    }
  }
}

Fact ControlAnalysis::before(const Instruction *at, const Storage *storage) const {
  auto found = entryFacts.find(at->getParent());
  if (found == entryFacts.end()) {
    return Fact{Fact::Quantum, nullptr};
  }
  std::vector<Fact> facts = found->second;
  for (const Instruction &inst : *at->getParent()) {
    if (&inst == at) {
      break;
    }
    transfer(inst, facts);
  }
  Fact fact = facts[indexOf(storage)];
  return fact.kind == Fact::Unvisited ? Fact{Fact::Quantum, nullptr} : fact;
}

bool ControlAnalysis::computeTaint() {
  quantumLoads.clear();
  for (Instruction &inst : instructions(function)) {
    auto *load = dyn_cast<LoadInst>(&inst);
    if (!load) {
      continue;
    }
    std::optional<Access> source = access(load->getPointerOperand());
    if (!source) {
      continue;
    }
    Fact fact = before(load, source->storage);
    bool classical = !taintAll && fact.kind == Fact::Known && dominators.dominates(fact.value, load);
    if (!classical) {
      quantumLoads.insert(load);
    }
  }
  std::set<const Value *> next(quantumLoads.begin(), quantumLoads.end());
  std::vector<const Value *> work(quantumLoads.begin(), quantumLoads.end());
  while (!work.empty()) {
    const Value *value = work.back();
    work.pop_back();
    for (const User *user : value->users()) {
      const auto *inst = dyn_cast<Instruction>(user);
      if (inst && !inst->getType()->isVoidTy() && next.insert(inst).second) {
        work.push_back(inst);
      }
    }
  }
  bool changed = next != taint;
  taint = std::move(next);
  return changed;
}

bool ControlAnalysis::isQuantumLoad(const Value *value) const { return quantumLoads.count(value) != 0; }

Instruction *ControlAnalysis::seed() const {
  for (BasicBlock *block : order) {
    for (PHINode &phi : block->phis()) {
      if (tainted(&phi)) {
        return &phi;
      }
    }
    Instruction *term = block->getTerminator();
    if (auto *branch = dyn_cast<BranchInst>(term); branch && branch->isConditional() &&
                                                   tainted(branch->getCondition())) {
      return term;
    }
    if (auto *switchInst = dyn_cast<SwitchInst>(term); switchInst && tainted(switchInst->getCondition())) {
      return term;
    }
  }
  return nullptr;
}

namespace {

// Whether `to` can execute after `from` without control re-entering the top of `barrier` in between.
bool reachesAvoiding(const Instruction *from, const Instruction *to, BasicBlock *barrier, const DominatorTree &dt) {
  const BasicBlock *fromBlock = from->getParent();
  const BasicBlock *toBlock = to->getParent();
  if (fromBlock == toBlock && from->comesBefore(to)) {
    return true;
  }
  if (toBlock == barrier) {
    return false; // reaching it again means entering the barrier at its top
  }
  SmallPtrSet<BasicBlock *, 4> exclude;
  exclude.insert(barrier);
  for (const BasicBlock *succ : successors(fromBlock)) {
    if (succ != barrier && isPotentiallyReachable(succ, toBlock, &exclude, &dt, nullptr)) {
      return true;
    }
  }
  return false;
}

} // namespace

bool ControlAnalysis::unchangedBetween(const Instruction *load, const Instruction *use, const Storage *storage) const {
  // The SSA value of `load` seen at `use` comes from its latest execution; re-entering the load's block
  // executes it again, so only paths that avoid re-entering that block matter.
  BasicBlock *barrier = const_cast<BasicBlock *>(load->getParent());
  for (const Instruction *mutation : storage->mutations) {
    if (reachesAvoiding(load, mutation, barrier, dominators) && reachesAvoiding(mutation, use, barrier, dominators)) {
      return false;
    }
  }
  return true;
}

} // namespace bennett

namespace bennett {

bool ControlAnalysis::memoryUnchangedBetween(const Instruction *from, const Instruction *use) const {
  BasicBlock *barrier = const_cast<BasicBlock *>(from->getParent());
  for (const Instruction &inst : instructions(function)) {
    if (!inst.mayWriteToMemory() || markerCall(&inst)) {
      continue;
    }
    if (const auto *store = dyn_cast<StoreInst>(&inst); store && access(store->getPointerOperand())) {
      continue;
    }
    if (reachesAvoiding(from, &inst, barrier, dominators) && reachesAvoiding(&inst, use, barrier, dominators)) {
      return false;
    }
  }
  return true;
}

} // namespace bennett
