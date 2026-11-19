#include "Classify.h"

#include "Aliasing.h"
#include "Diagnostic.h"
#include "Markers.h"
#include "Outline.h"
#include "Passes.h"
#include "Taint.h"

#include <llvm/ADT/PostOrderIterator.h>
#include <llvm/Analysis/PostDominators.h>
#include <llvm/Analysis/ValueTracking.h>
#include <llvm/IR/Dominators.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>
#include <llvm/Transforms/Utils/Cloning.h>

#include <map>
#include <memory>
#include <set>
#include <string>
#include <vector>

using namespace llvm;

namespace bennett {

namespace {

constexpr OperationClass allClasses[] = {OperationClass::Read,        OperationClass::HeldStore,
                                         OperationClass::QubitStore,  OperationClass::AppliedCall,
                                         OperationClass::HeldMemory,  OperationClass::AtomicUpdate};

void setClass(Instruction &inst, OperationClass kind) {
  LLVMContext &context = inst.getContext();
  inst.setMetadata(classMetadata, MDNode::get(context, {MDString::get(context, className(kind))}));
}

// SROA turns a local variable into values and folds a join of equal values: after `if (q) t = n; else t = n;` it
// passes `n` itself on, and the choice, which depends on a qubit, is gone (by the text, `t` depends on a qubit
// after it). So before SROA each integer stored into a local variable goes through a call of its own of a
// declaration that returns its argument (`bennett.keep.<n>`, no memory, no other effect); the values joined are
// then distinct and the join stays a phi, which the classification sees. The calls are removed after it.
// Pointers are not wrapped: the analysis follows a pointer to its object through a store and a load SROA removes.
constexpr const char *keeperPrefix = "bennett.keep.";

bool isKeeper(const CallBase &call) {
  const Function *callee = call.getCalledFunction();
  return callee && callee->getName().starts_with(keeperPrefix);
}

// Whether `alloca` is a qubit variable a marker names directly (SROA never promotes it: its address goes to the
// marker); its stores are left as they are, so the known-value analysis sees the stored values themselves.
bool namedByMarker(const AllocaInst &alloca) {
  if (alloca.getMetadata(qubitMetadata)) {
    return true;
  }
  std::vector<const Value *> work{&alloca};
  while (!work.empty()) {
    const Value *pointer = work.back();
    work.pop_back();
    for (const User *user : pointer->users()) {
      if (isa<GetElementPtrInst>(user)) {
        work.push_back(user);
      } else if (markerCall(user)) {
        return true;
      }
    }
  }
  return false;
}

void keepStoredValues(Module &module) {
  std::map<Type *, Function *> keepers;
  std::map<const AllocaInst *, bool> named;
  for (Function &function : module) {
    for (Instruction &inst : instructions(function)) {
      auto *store = dyn_cast<StoreInst>(&inst);
      Type *type = store ? store->getValueOperand()->getType() : nullptr;
      const auto *local = store ? dyn_cast<AllocaInst>(getUnderlyingObject(store->getPointerOperand())) : nullptr;
      if (!local || !type->isIntOrIntVectorTy()) {
        continue;
      }
      auto [cached, fresh] = named.try_emplace(local, false);
      if (fresh) {
        cached->second = namedByMarker(*local);
      }
      if (cached->second) {
        continue;
      }
      Function *&keeper = keepers[type];
      if (!keeper) {
        keeper = Function::Create(FunctionType::get(type, {type}, false), GlobalValue::ExternalLinkage,
                                  keeperPrefix + std::to_string(keepers.size()), module);
        keeper->setDoesNotAccessMemory();
        keeper->setDoesNotThrow();
        keeper->setWillReturn();
      }
      store->setOperand(0, CallInst::Create(keeper, {store->getValueOperand()}, "", store));
    }
  }
}

void releaseStoredValues(Module &module) {
  std::vector<Function *> keepers;
  for (Function &function : module) {
    if (function.getName().starts_with(keeperPrefix)) {
      keepers.push_back(&function);
    }
  }
  for (Function *keeper : keepers) {
    for (User *user : make_early_inc_range(keeper->users())) {
      auto *call = cast<CallInst>(user);
      call->replaceAllUsesWith(call->getArgOperand(0));
      call->eraseFromParent();
    }
    keeper->eraseFromParent();
  }
}

// What is known about a qubit variable at a point, as the front's later analysis (Taint.h) decides it: the
// fresh 0, one ordinary value stored whole, or a measurement; anything else is Quantum.
struct Fact {
  enum Kind { Unvisited, Known, Quantum };
  Kind kind = Unvisited;
  const Value *value = nullptr;
};

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
  for (size_t i = 0; i < a.size(); ++i) {
    if (a[i].kind != b[i].kind || a[i].value != b[i].value) {
      return false;
    }
  }
  return true;
}

class Classifier {
public:
  Classifier(Module &module, bool taintAll) : module(module), taintAll(taintAll), aliasing(module) {}

  // Runs the analysis to its fixed point.
  void run();
  // Refuses by name what the classes cannot cover (after run).
  void refuseUnclassified();
  // Refuses by name a call whose argument depends on a qubit and that the front's calling-convention lowering
  // (Abi.h, Normalise.h) would pass in memory or drop (after run, on the module before that lowering).
  void refuseArgumentsLoweredAway();
  std::map<Instruction *, OperationClass> classes;

private:
  Module &module;
  bool taintAll;
  Aliasing aliasing;
  std::set<const Function *> control;           // functions with gates (Outline.h)
  std::set<const Value *> qubitObjects;         // qubit variables, and pointer parameters markers act through
  std::map<const AllocaInst *, unsigned> tracked; // qubit variables whose every use the known-value analysis sees
  std::set<const LoadInst *> knownLoads;        // loads of a tracked variable while it is known
  std::set<const Value *> tainted;
  Aliasing::Objects held;                       // objects that may hold qubits
  Aliasing::Objects qubitVariables;             // the qubit variables (allocas among qubitObjects)
  std::set<const Function *> returnsTainted;
  std::map<const Function *, std::unique_ptr<PostDominatorTree>> postDominators;
  std::set<const BasicBlock *> underQubit;      // blocks a conditional jump that depends on a qubit decides
  std::set<const Instruction *> taintedJumps;
  bool indirectTainted = false; // a function whose address is taken may return a value that depends on a qubit

  void findQubitObjects();
  void findTracked();
  void rewriteSmallMemoryIntrinsics();
  bool computeKnownLoads();
  void knownLoadsOf(Function &function, const std::vector<const AllocaInst *> &variables);
  bool step();
  bool markRegion(Instruction &jump);

  // The qubit variable or pointer parameter `pointer` points into, if any (several: the first found).
  bool isQubitPointer(const Value *pointer) const {
    SmallVector<const Value *, 4> bases;
    getUnderlyingObjects(pointer, bases, nullptr, 0);
    return any_of(bases, [&](const Value *base) { return qubitObjects.count(base) != 0; });
  }
  // Whether every object `pointer` may point into is a qubit variable (a parameter may also be passed memory).
  bool onlyQubitVariables(const Value *pointer) const {
    SmallVector<const Value *, 4> bases;
    getUnderlyingObjects(pointer, bases, nullptr, 0);
    return all_of(bases, [&](const Value *base) { return isa<AllocaInst>(base) && qubitObjects.count(base); });
  }
  bool reachesHeld(const Function &function, const Value *pointer) const {
    return aliasing.mayReach(function, pointer, held);
  }
  // Whether `pointer` may reach bytes whose value depends on a qubit: memory that may hold qubits, or a qubit
  // variable (whose address may have been returned, stored or passed on, so that the front cannot follow it).
  bool reachesQubitBytes(const Function &function, const Value *pointer) const {
    return reachesHeld(function, pointer) || aliasing.mayReach(function, pointer, qubitVariables);
  }
  bool anyPointerReachesHeld(const CallBase &call) const {
    return any_of(call.args(), [&](const Use &arg) {
      return arg->getType()->isPointerTy() && reachesHeld(*call.getFunction(), arg.get());
    });
  }
  bool anyPointerReachesQubitBytes(const CallBase &call) const {
    return any_of(call.args(), [&](const Use &arg) {
      return arg->getType()->isPointerTy() && reachesQubitBytes(*call.getFunction(), arg.get());
    });
  }
  bool taint(const Value *value) { return !value->getType()->isVoidTy() && tainted.insert(value).second; }
  bool isTainted(const Value *value) const { return tainted.count(value) != 0; }
  bool anyArgumentTainted(const CallBase &call) const {
    return any_of(call.args(), [&](const Use &arg) { return isTainted(arg.get()); });
  }
  bool hold(const Value *pointer) {
    size_t before = held.size();
    aliasing.addObjectsOf(pointer, held);
    return held.size() != before;
  }
  // Classifies `inst`; a store found later to depend on a qubit becomes a qubit store.
  bool classify(Instruction &inst, OperationClass kind) {
    auto [found, fresh] = classes.try_emplace(&inst, kind);
    if (!fresh && found->second != kind && kind == OperationClass::QubitStore) {
      found->second = kind;
      return true;
    }
    return fresh;
  }
};

// Qubit variables (allocas a marker acts on, and the front's own, tagged), pointer parameters of helpers that
// pass them on to a marker, through any number of calls, and pointer parameters some call passes a qubit
// variable's address. A global variable a marker acts on may hold qubits; so
// may whatever a pointer the front cannot follow points to.
void Classifier::findQubitObjects() {
  auto collect = [&](const Value *pointer) {
    bool changed = false;
    if (std::optional<Marker> named = markerCall(pointer); named && named->kind == MarkerKind::Bit) {
      return changed; // a qubit of a variable, which the call of bennett_bit names
    }
    SmallVector<const Value *, 4> bases;
    getUnderlyingObjects(pointer, bases, nullptr, 0);
    for (const Value *base : bases) {
      if (isa<AllocaInst>(base) || isa<Argument>(base)) {
        changed |= qubitObjects.insert(base).second;
      } else {
        changed |= hold(pointer); // a gate on memory at an address
      }
    }
    return changed;
  };
  for (bool changed = true; changed;) {
    changed = false;
    for (Function &function : module) {
      for (Instruction &inst : instructions(function)) {
        if (auto *alloca = dyn_cast<AllocaInst>(&inst); alloca && alloca->getMetadata(qubitMetadata)) {
          changed |= qubitObjects.insert(alloca).second;
          continue;
        }
        auto *call = dyn_cast<CallBase>(&inst);
        if (!call || call->arg_size() == 0) {
          continue;
        }
        if (std::optional<Marker> marker = markerCall(call)) {
          if (marker->kind != MarkerKind::MeasureBytes) {
            changed |= collect(call->getArgOperand(0));
          }
          continue;
        }
        Function *callee = call->getCalledFunction();
        if (!callee || callee->isDeclaration()) {
          continue;
        }
        for (Argument &param : callee->args()) {
          if (param.getArgNo() >= call->arg_size()) {
            continue;
          }
          if (qubitObjects.count(&param)) {
            changed |= collect(call->getArgOperand(param.getArgNo()));
          } else if (isQubitPointer(call->getArgOperand(param.getArgNo()))) {
            // A qubit variable's address handed to a helper: what the helper reads through it is a qubit's value.
            changed |= qubitObjects.insert(&param).second;
          }
        }
      }
    }
  }
}

// A qubit variable is tracked when every use of its address, through constant offsets, is a load, the address
// operand of a store, a marker or a lifetime marker.
void Classifier::findTracked() {
  const DataLayout &layout = module.getDataLayout();
  for (const Value *object : qubitObjects) {
    const auto *alloca = dyn_cast<AllocaInst>(object);
    std::optional<TypeSize> size = alloca ? alloca->getAllocationSize(layout) : std::nullopt;
    if (!alloca || !alloca->isStaticAlloca() || !size || size->isScalable()) {
      continue;
    }
    bool simple = true;
    std::vector<const Value *> work{alloca};
    while (!work.empty() && simple) {
      const Value *pointer = work.back();
      work.pop_back();
      for (const User *user : pointer->users()) {
        if (const auto *gep = dyn_cast<GetElementPtrInst>(user); gep && gep->hasAllConstantIndices() &&
                                                                 gep->getPointerOperand() == pointer) {
          work.push_back(gep);
        } else if (const auto *store = dyn_cast<StoreInst>(user)) {
          simple = simple && store->getValueOperand() != pointer;
        } else if (const auto *intrinsic = dyn_cast<IntrinsicInst>(user)) {
          simple = simple && intrinsic->isLifetimeStartOrEnd();
        } else {
          simple = simple && (isa<LoadInst>(user) || markerCall(user));
        }
      }
    }
    if (simple) {
      tracked.emplace(alloca, unsigned(size->getFixedValue() * 8));
    }
  }
}

// The tracked variable `pointer` points into at a constant offset, if any.
const AllocaInst *trackedBase(const std::map<const AllocaInst *, unsigned> &tracked, const Value *pointer,
                              const DataLayout &layout, uint64_t &offset) {
  APInt delta(64, 0);
  const auto *base = dyn_cast<AllocaInst>(pointer->stripAndAccumulateConstantOffsets(layout, delta, true));
  offset = delta.getZExtValue();
  return base && tracked.count(base) ? base : nullptr;
}

// The known-value dataflow of the front's later analysis (Taint.h), over the tracked variables of `function`.
void Classifier::knownLoadsOf(Function &function, const std::vector<const AllocaInst *> &variables) {
  const DataLayout &layout = module.getDataLayout();
  std::map<const AllocaInst *, size_t> index;
  for (size_t i = 0; i < variables.size(); ++i) {
    index[variables[i]] = i;
  }
  auto transfer = [&](const Instruction &inst, std::vector<Fact> &facts) {
    uint64_t offset = 0;
    if (const auto *store = dyn_cast<StoreInst>(&inst)) {
      if (const AllocaInst *base = trackedBase(tracked, store->getPointerOperand(), layout, offset);
          base && index.count(base)) {
        const Value *value = store->getValueOperand();
        bool whole = offset == 0 && layout.getTypeStoreSizeInBits(value->getType()) == tracked.at(base);
        facts[index.at(base)] = whole && !isTainted(value) ? Fact{Fact::Known, value} : Fact{Fact::Quantum, nullptr};
      }
      return;
    }
    std::optional<Marker> marker = markerCall(&inst);
    if (!marker || marker->kind == MarkerKind::Bit) {
      return;
    }
    const Value *pointer = cast<CallBase>(inst).getArgOperand(0);
    if (std::optional<Marker> named = markerCall(pointer); named && named->kind == MarkerKind::Bit) {
      pointer = cast<CallBase>(pointer)->getArgOperand(0);
    }
    if (const AllocaInst *base = trackedBase(tracked, pointer, layout, offset); base && index.count(base)) {
      bool measured = marker->kind == MarkerKind::Measure || marker->kind == MarkerKind::MeasureBytes;
      facts[index.at(base)] = measured ? Fact{Fact::Known, &inst} : Fact{Fact::Quantum, nullptr};
    }
  };
  std::vector<Fact> initial;
  for (const AllocaInst *variable : variables) {
    initial.push_back(Fact{Fact::Known, ConstantInt::get(IntegerType::get(module.getContext(), tracked.at(variable)), 0)});
  }
  ReversePostOrderTraversal<Function *> order(&function);
  std::map<const BasicBlock *, std::vector<Fact>> entryFacts, exitFacts;
  for (bool changed = true; changed;) {
    changed = false;
    for (BasicBlock *block : order) {
      std::vector<Fact> facts(variables.size());
      if (block == &function.getEntryBlock()) {
        facts = initial;
      } else {
        for (BasicBlock *pred : predecessors(block)) {
          if (auto found = exitFacts.find(pred); found != exitFacts.end()) {
            for (size_t i = 0; i < facts.size(); ++i) {
              facts[i] = meet(facts[i], found->second[i]);
            }
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
  DominatorTree dominators(function);
  for (BasicBlock *block : order) {
    std::vector<Fact> facts = entryFacts[block];
    for (Instruction &inst : *block) {
      uint64_t offset = 0;
      const auto *load = dyn_cast<LoadInst>(&inst);
      const AllocaInst *base = load ? trackedBase(tracked, load->getPointerOperand(), layout, offset) : nullptr;
      if (base && index.count(base)) {
        const Fact &fact = facts[index.at(base)];
        if (fact.kind == Fact::Known && dominators.dominates(fact.value, load)) {
          knownLoads.insert(load);
        }
      }
      transfer(inst, facts);
    }
  }
}

// Recomputes which loads of tracked variables are known. Returns whether that changed.
bool Classifier::computeKnownLoads() {
  std::set<const LoadInst *> before = std::move(knownLoads);
  knownLoads.clear();
  if (taintAll) {
    return !before.empty();
  }
  std::map<Function *, std::vector<const AllocaInst *>> byFunction;
  for (const auto &[variable, bits] : tracked) {
    byFunction[const_cast<Function *>(variable->getFunction())].push_back(variable);
  }
  for (auto &[function, variables] : byFunction) {
    knownLoadsOf(*function, variables);
  }
  return knownLoads != before;
}

// The blocks a conditional jump or switch that depends on a qubit decides between: from its successors up to its
// immediate post-dominator (all that follows when it has none).
bool Classifier::markRegion(Instruction &jump) {
  if (!taintedJumps.insert(&jump).second) {
    return false;
  }
  Function &function = *jump.getFunction();
  std::unique_ptr<PostDominatorTree> &tree = postDominators[&function];
  if (!tree) {
    tree = std::make_unique<PostDominatorTree>(function);
  }
  DomTreeNode *node = tree->getNode(jump.getParent());
  BasicBlock *join = node && node->getIDom() ? node->getIDom()->getBlock() : nullptr;
  std::vector<BasicBlock *> work(succ_begin(jump.getParent()), succ_end(jump.getParent()));
  while (!work.empty()) {
    BasicBlock *next = work.back();
    work.pop_back();
    if (next == join || !underQubit.insert(next).second) {
      continue;
    }
    work.insert(work.end(), succ_begin(next), succ_end(next));
  }
  return true;
}

bool Classifier::step() {
  bool changed = false;
  for (Function &function : module) {
    for (Instruction &inst : instructions(function)) {
      bool under = underQubit.count(inst.getParent()) != 0;
      if (auto *load = dyn_cast<LoadInst>(&inst)) {
        const Value *pointer = load->getPointerOperand();
        if (isQubitPointer(pointer)) {
          if (!knownLoads.count(load)) {
            changed |= taint(load);
          }
          if (onlyQubitVariables(pointer)) {
            continue;
          }
        }
        if (reachesQubitBytes(function, pointer)) {
          changed |= classify(inst, OperationClass::Read);
          changed |= taint(load);
        }
        if (isTainted(pointer)) {
          changed |= taint(load);
        }
      } else if (auto *store = dyn_cast<StoreInst>(&inst)) {
        const Value *pointer = store->getPointerOperand();
        if (onlyQubitVariables(pointer)) {
          continue; // a qubit variable: the front's own analyses decide, and the known values above follow it
        }
        if (isTainted(store->getValueOperand()) || isTainted(pointer) || under) {
          changed |= classify(inst, OperationClass::QubitStore);
          changed |= hold(pointer);
        } else if (reachesHeld(function, pointer)) {
          changed |= classify(inst, OperationClass::HeldStore);
        }
      } else if (isa<AtomicRMWInst>(inst) || isa<AtomicCmpXchgInst>(inst)) {
        const Value *pointer = isa<AtomicRMWInst>(inst) ? cast<AtomicRMWInst>(inst).getPointerOperand()
                                                        : cast<AtomicCmpXchgInst>(inst).getPointerOperand();
        bool depends = any_of(inst.operands(), [&](const Use &use) { return isTainted(use.get()); }) || under;
        if (depends) {
          changed |= hold(pointer);
        }
        if (depends || reachesHeld(function, pointer)) {
          changed |= classify(inst, OperationClass::AtomicUpdate);
          changed |= taint(&inst);
        }
      } else if (auto *call = dyn_cast<CallBase>(&inst)) {
        if (isKeeper(*call)) {
          if (anyArgumentTainted(*call)) {
            changed |= taint(call); // the stored value itself (keepStoredValues)
          }
          continue;
        }
        if (markerCall(call)) {
          continue; // a measurement's result is ordinary; the qubit a gate names is no value
        }
        bool depends = anyArgumentTainted(*call);
        Function *callee = call->getCalledFunction();
        if (call->isInlineAsm()) {
          // Only an empty barrier reaches the emitter (Normalise.h): its outputs are its inputs.
          if (depends) {
            changed |= taint(call);
          }
        } else if (isa<MemIntrinsic>(call)) {
          // Its destination holds qubits when what it writes depends on one, or is copied from bytes that may: memory
          // that may hold qubits, or a qubit variable.
          const auto *transfer = dyn_cast<MemTransferInst>(call);
          bool qubitBytes = transfer && (isQubitPointer(transfer->getRawSource()) ||
                                         reachesQubitBytes(function, transfer->getRawSource()));
          if (depends || under || qubitBytes || anyPointerReachesHeld(*call)) {
            changed |= hold(call->getArgOperand(0));
            changed |= classify(inst, OperationClass::HeldMemory);
          }
        } else if (callee && callee->isIntrinsic()) {
          if (depends || anyPointerReachesQubitBytes(*call)) {
            changed |= taint(call);
          }
        } else if (depends && control.count(callee)) {
          // A function with gates runs in the control program: its parameters depend on a qubit.
          for (Argument &param : callee->args()) {
            if (param.getArgNo() < call->arg_size() && isTainted(call->getArgOperand(param.getArgNo()))) {
              changed |= taint(&param);
            }
          }
          if (returnsTainted.count(callee)) {
            changed |= taint(call);
          }
        } else if (depends && isa<CallInst>(call)) {
          changed |= classify(inst, OperationClass::AppliedCall);
          changed |= taint(call);
        } else if ((callee && returnsTainted.count(callee)) || (!callee && indirectTainted) ||
                   anyPointerReachesQubitBytes(*call)) {
          changed |= taint(call);
        }
      } else if (auto *ret = dyn_cast<ReturnInst>(&inst)) {
        if (ret->getReturnValue() && isTainted(ret->getReturnValue()) && returnsTainted.insert(&function).second) {
          changed = true;
          indirectTainted = indirectTainted || function.hasAddressTaken();
        }
      } else if (auto *branch = dyn_cast<BranchInst>(&inst)) {
        if (branch->isConditional() && isTainted(branch->getCondition())) {
          changed |= markRegion(inst);
        }
      } else if (auto *switchInst = dyn_cast<SwitchInst>(&inst)) {
        if (isTainted(switchInst->getCondition())) {
          changed |= markRegion(inst);
        }
      } else if (!inst.getType()->isVoidTy()) {
        bool depends = any_of(inst.operands(), [&](const Use &use) { return isTainted(use.get()); });
        if (auto *phi = dyn_cast<PHINode>(&inst)) {
          for (BasicBlock *incoming : phi->blocks()) {
            depends = depends || underQubit.count(incoming) || taintedJumps.count(incoming->getTerminator());
          }
        }
        if (depends) {
          changed |= taint(&inst);
        }
      }
    }
  }
  return changed;
}

// What the optimiser could change and no class covers is refused by name.
void Classifier::refuseUnclassified() {
  for (Function &function : module) {
    for (Instruction &inst : instructions(function)) {
      if (auto *call = dyn_cast<CallBase>(&inst); call && !isa<CallInst>(call) && anyArgumentTainted(*call)) {
        notHandled(std::string("`") + inst.getOpcodeName() +
                       "` with an argument that depends on a qubit (not built: only a plain call can be applied)",
                   &inst);
      }
      auto *intrinsic = dyn_cast<IntrinsicInst>(&inst);
      // Lifetime markers and the stack pointer's save and restore name no bytes they read or write.
      bool namesNoBytes = intrinsic && (intrinsic->isLifetimeStartOrEnd() ||
                                        intrinsic->getIntrinsicID() == Intrinsic::stacksave ||
                                        intrinsic->getIntrinsicID() == Intrinsic::stackrestore);
      if (intrinsic && !isa<MemIntrinsic>(intrinsic) && !namesNoBytes &&
          !intrinsic->getMemoryEffects().getWithoutLoc(IRMemLocation::InaccessibleMem).doesNotAccessMemory() &&
          anyPointerReachesHeld(*intrinsic)) {
        notHandled("the intrinsic @" + intrinsic->getCalledFunction()->getName().str() +
                       " on memory that may hold qubits (not built: the front cannot keep it where C has it)",
                   &inst);
      }
      if (auto *argument = dyn_cast<VAArgInst>(&inst);
          argument && reachesHeld(function, argument->getPointerOperand())) {
        notHandled("`va_arg` on memory that may hold qubits (not built: the front cannot keep it where C has it)",
                   &inst);
      }
    }
  }
}

// A memory intrinsic of a constant, small length becomes an integer load and store, as LLVM's InstCombine rewrites it
// in the optimiser (memcpy and memmove of 1, 2, 4 or 8 bytes become a load and a store, memset a store of the byte
// repeated); sealed as a call (HeldMemory), the intrinsic would reach the front unrewritten and be expanded into a
// loop, which ordinary conditions guard. A copy out of a qubit variable of any length up to its 8 bytes is
// rewritten too: sealed as a call it would keep the variable's address in ordinary code, which the front refuses.
// The loads and stores are then classified like any other.
void Classifier::rewriteSmallMemoryIntrinsics() {
  std::vector<MemIntrinsic *> rewritten;
  for (Function &function : module) {
    for (Instruction &inst : instructions(function)) {
      auto *intrinsic = dyn_cast<MemIntrinsic>(&inst);
      auto *length = intrinsic ? dyn_cast<ConstantInt>(intrinsic->getLength()) : nullptr;
      if (!length || intrinsic->isVolatile() || length->getZExtValue() == 0 ||
          length->getZExtValue() > sizeof(uint64_t)) {
        continue;
      }
      uint64_t bytes = length->getZExtValue();
      bool wordSized = bytes == 1 || bytes == 2 || bytes == 4 || bytes == 8;
      auto *transfer = dyn_cast<MemTransferInst>(intrinsic);
      if (wordSized || (transfer && onlyQubitVariables(transfer->getRawSource()))) {
        rewritten.push_back(intrinsic);
      }
    }
  }
  for (MemIntrinsic *intrinsic : rewritten) {
    IRBuilder<> b(intrinsic);
    uint64_t bytes = cast<ConstantInt>(intrinsic->getLength())->getZExtValue();
    IntegerType *type = b.getIntNTy(unsigned(bytes * 8));
    Value *value;
    if (auto *transfer = dyn_cast<MemTransferInst>(intrinsic)) {
      LoadInst *load = b.CreateAlignedLoad(type, transfer->getRawSource(), transfer->getSourceAlign().valueOrOne());
      load->setDebugLoc(intrinsic->getDebugLoc());
      value = load;
    } else {
      // The byte repeated in every byte of the word.
      APInt ones = APInt::getSplat(unsigned(bytes * 8), APInt(8, 1));
      value = b.CreateMul(b.CreateZExt(cast<MemSetInst>(intrinsic)->getValue(), type), ConstantInt::get(type, ones));
    }
    StoreInst *store = b.CreateAlignedStore(value, intrinsic->getRawDest(), intrinsic->getDestAlign().valueOrOne());
    store->setDebugLoc(intrinsic->getDebugLoc());
    intrinsic->eraseFromParent();
  }
}

void Classifier::run() {
  control = controlFunctions(module, /*refuseAddressTaken=*/false); // the optimiser may still remove the address
  findQubitObjects();
  for (const Value *object : qubitObjects) {
    if (isa<AllocaInst>(object)) {
      qubitVariables.insert(object);
    }
  }
  rewriteSmallMemoryIntrinsics();
  findTracked();
  computeKnownLoads();
  for (bool changed = true; changed;) {
    changed = step();
    changed = computeKnownLoads() || changed;
  }
}

void Classifier::refuseArgumentsLoweredAway() {
  for (Function &function : module) {
    for (Instruction &inst : instructions(function)) {
      auto *call = dyn_cast<CallBase>(&inst);
      if (!call || markerCall(call) || call->isInlineAsm() || isa<IntrinsicInst>(call) || isKeeper(*call)) {
        continue;
      }
      const Function *callee = call->getCalledFunction();
      FunctionType *type = call->getFunctionType();
      for (unsigned i = 0; i < call->arg_size(); ++i) {
        const Value *arg = call->getArgOperand(i);
        const char *how = nullptr;
        if (i >= type->getNumParams() && isTainted(arg)) {
          how = "a variadic argument, which the front passes in memory";
        } else if (call->isByValArgument(i) && reachesHeld(function, arg)) {
          how = "an argument passed by value, which is a copy in memory";
        } else if ((arg->getType()->isVectorTy() || arg->getType()->isAggregateType()) && isTainted(arg)) {
          how = "a vector or aggregate argument, which the front passes in memory";
        } else if (callee && callee->getFunctionType() != type && isTainted(arg)) {
          how = "an argument of a call through another prototype, which the front may drop";
        }
        if (how) {
          notHandled(std::string("a call with ") + how + ", that depends on a qubit (not built: keep the argument's "
                                                          "dependence through the lowering and apply the call)",
                     &inst);
        }
      }
    }
  }
}

} // namespace

const char *className(OperationClass kind) {
  switch (kind) {
  case OperationClass::Read:
    return "read";
  case OperationClass::HeldStore:
    return "held-store";
  case OperationClass::QubitStore:
    return "qubit-store";
  case OperationClass::AppliedCall:
    return "applied-call";
  case OperationClass::HeldMemory:
    return "held-memory";
  case OperationClass::AtomicUpdate:
    return "atomic-update";
  }
  fail("internal: an operation class without a name");
}

const char *classDescription(OperationClass kind) {
  switch (kind) {
  case OperationClass::Read:
    return "a read of memory that may hold qubits";
  case OperationClass::HeldStore:
    return "a store of an ordinary value into memory that may hold qubits";
  case OperationClass::QubitStore:
    return "a store that depends on a qubit";
  case OperationClass::AppliedCall:
    return "a call with an argument that depends on a qubit";
  case OperationClass::HeldMemory:
    return "a memory intrinsic on memory that may hold qubits";
  case OperationClass::AtomicUpdate:
    return "an atomic update of memory that may hold qubits";
  }
  fail("internal: an operation class without a description");
}

std::optional<OperationClass> classOf(const Instruction &inst) {
  const MDNode *node = inst.getMetadata(classMetadata);
  if (!node) {
    return std::nullopt;
  }
  StringRef name = cast<MDString>(node->getOperand(0))->getString();
  for (OperationClass kind : allClasses) {
    if (name == className(kind)) {
      return kind;
    }
  }
  fail("internal: an unknown operation class `" + name.str() + "`");
}

void declassify(Instruction &inst) { inst.setMetadata(classMetadata, nullptr); }

void refuseAppliedCallsRunOrdinarily(const Module &module, const std::set<const Function *> &control,
                                     const std::vector<const CallBase *> &ordinaryCalls) {
  std::set<const Function *> ordinary;
  std::vector<const Function *> work;
  auto add = [&](const Function *function) {
    if (function && !function->isDeclaration() && !control.count(function) && ordinary.insert(function).second) {
      work.push_back(function);
    }
  };
  auto addAddressTaken = [&] {
    for (const Function &function : module) {
      if (function.hasAddressTaken()) {
        add(&function);
      }
    }
  };
  // What the host may call, what may be called through a pointer, and what the control program calls as ordinary
  // code; then everything those call.
  for (const Function &function : module) {
    if (!function.hasLocalLinkage()) {
      add(&function);
    }
  }
  addAddressTaken();
  for (const CallBase *call : ordinaryCalls) {
    add(call->getCalledFunction());
  }
  while (!work.empty()) {
    const Function *function = work.back();
    work.pop_back();
    for (const Instruction &inst : instructions(*function)) {
      if (const auto *call = dyn_cast<CallBase>(&inst)) {
        add(call->getCalledFunction());
      }
    }
  }
  for (const Function *function : ordinary) {
    for (const Instruction &inst : instructions(*function)) {
      if (classOf(inst) == OperationClass::AppliedCall) {
        notHandled(std::string(classDescription(OperationClass::AppliedCall)) +
                       " in @" + function->getName().str() +
                       ", which runs as ordinary code (not built: apply the call where it stands in ordinary code)",
                   &inst);
      }
    }
  }
}

void requireAppliedCallKept(const CallBase &call, bool dependsNow) {
  if (!dependsNow && classOf(call) == OperationClass::AppliedCall) {
    notHandled(std::string(classDescription(OperationClass::AppliedCall)) +
                   " before LLVM's optimiser, whose arguments are ordinary after it (not built: apply it by its "
                   "class, not as an ordinary call)",
               &call);
  }
}

void refuseQubitArgumentsLoweredAway(const Module &module, bool taintAll) {
  // On a copy after SROA, as the classification sees the program; the module itself is lowered unchanged.
  std::unique_ptr<Module> copy = CloneModule(module);
  keepStoredValues(*copy);
  promoteLocals(*copy);
  Classifier classifier(*copy, taintAll);
  classifier.run();
  classifier.refuseArgumentsLoweredAway();
}

void classify(Module &module, bool taintAll) {
  keepStoredValues(module);
  promoteLocals(module);
  Classifier classifier(module, taintAll);
  classifier.run();
  classifier.refuseUnclassified();
  for (auto &[inst, kind] : classifier.classes) {
    setClass(*inst, kind);
  }
  releaseStoredValues(module);
}

void splitClassifiedVectors(Module &module) {
  const DataLayout &layout = module.getDataLayout();
  std::vector<Instruction *> accesses;
  for (Function &function : module) {
    for (Instruction &inst : instructions(function)) {
      Type *type = isa<LoadInst>(inst)    ? inst.getType()
                   : isa<StoreInst>(inst) ? cast<StoreInst>(inst).getValueOperand()->getType()
                                          : nullptr;
      auto *vector = dyn_cast_or_null<FixedVectorType>(type);
      // As the Scalarizer: only simple accesses, of elements whose size in memory is their size.
      if (vector && classOf(inst) && !inst.isAtomic() &&
          !(isa<LoadInst>(inst) ? cast<LoadInst>(inst).isVolatile() : cast<StoreInst>(inst).isVolatile()) &&
          layout.typeSizeEqualsStoreSize(vector->getElementType()) &&
          layout.getTypeSizeInBits(vector->getElementType()) % 8 == 0) {
        accesses.push_back(&inst);
      }
    }
  }
  for (Instruction *inst : accesses) {
    OperationClass kind = *classOf(*inst);
    IRBuilder<> b(inst);
    Value *pointer = getLoadStorePointerOperand(inst);
    Align align = getLoadStoreAlignment(inst);
    auto *vector = cast<FixedVectorType>(getLoadStoreType(inst));
    Type *element = vector->getElementType();
    uint64_t size = layout.getTypeStoreSize(element).getFixedValue();
    Value *whole = PoisonValue::get(vector);
    for (unsigned i = 0; i < vector->getNumElements(); ++i) {
      // A vector's elements lie side by side in memory at their store size, which may be less than their
      // allocation size (an i24 takes 3 bytes in a vector, 4 alone).
      Value *address = b.CreateConstInBoundsGEP1_64(b.getInt8Ty(), pointer, i * size);
      Align at = commonAlignment(align, i * size);
      Instruction *part;
      if (isa<LoadInst>(inst)) {
        part = b.CreateAlignedLoad(element, address, at, inst->getName() + "." + Twine(i));
        whole = b.CreateInsertElement(whole, part, b.getInt32(i));
      } else {
        Value *value = b.CreateExtractElement(cast<StoreInst>(inst)->getValueOperand(), b.getInt32(i));
        part = b.CreateAlignedStore(value, address, at);
      }
      part->setDebugLoc(inst->getDebugLoc());
      setClass(*part, kind);
    }
    if (isa<LoadInst>(inst)) {
      whole->takeName(inst);
      inst->replaceAllUsesWith(whole);
    }
    inst->eraseFromParent();
  }
}

} // namespace bennett
