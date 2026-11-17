#include "HeldBytes.h"

#include "Diagnostic.h"
#include "Markers.h"
#include "Taint.h"

#include <llvm/Analysis/AssumptionCache.h>
#include <llvm/Analysis/LoopInfo.h>
#include <llvm/Analysis/ScalarEvolution.h>
#include <llvm/Analysis/TargetLibraryInfo.h>
#include <llvm/Analysis/ValueTracking.h>
#include <llvm/IR/ConstantRange.h>
#include <llvm/IR/Constants.h>
#include <llvm/IR/Dominators.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/Module.h>
#include <llvm/TargetParser/Triple.h>

#include <algorithm>
#include <map>
#include <optional>

using namespace llvm;

namespace bennett {

namespace {

// The bytes one shadow holds: one register, the device program's widest value.
constexpr uint64_t maxHeldBytes = 8;

constexpr const char *exchangeName = "bennett.exchange";

// Byte offsets [begin, end) from an object's start.
struct Range {
  uint64_t begin;
  uint64_t end;
  bool overlaps(const Range &other) const { return begin < other.end && other.begin < end; }
  bool within(const Range &other) const { return other.begin <= begin && end <= other.end; }
};

// The object `pointer` points into, if the front can name it: a global variable that is not constant, a pointer
// argument, or a local of fixed size. Its address is then an ordinary value everywhere in the function.
Value *objectOf(Value *pointer) {
  Value *base = const_cast<Value *>(getUnderlyingObject(pointer, 0));
  if (auto *global = dyn_cast<GlobalVariable>(base); global && !global->isConstant()) {
    return base;
  }
  if (isa<Argument>(base) && base->getType()->isPointerTy()) {
    return base;
  }
  if (auto *alloca = dyn_cast<AllocaInst>(base); alloca && alloca->isStaticAlloca()) {
    return base;
  }
  return nullptr;
}

std::string objectName(const Value *object) {
  if (isa<GlobalVariable>(object)) {
    return "@" + object->getName().str();
  }
  return object->hasName() ? "%" + object->getName().str() : "a pointer argument";
}

std::string describe(const Value *object, const Range &range) {
  uint64_t count = range.end - range.begin;
  return std::to_string(count) + (count == 1 ? " byte at " : " bytes at ") + objectName(object) + "+" +
         std::to_string(range.begin);
}

Type *accessType(const Instruction &inst) {
  if (const auto *load = dyn_cast<LoadInst>(&inst)) {
    return load->getType();
  }
  return cast<StoreInst>(inst).getValueOperand()->getType();
}

Value *pointerOf(Instruction &inst) {
  if (auto *load = dyn_cast<LoadInst>(&inst)) {
    return load->getPointerOperand();
  }
  if (auto *store = dyn_cast<StoreInst>(&inst)) {
    return store->getPointerOperand();
  }
  return nullptr;
}

class Holder {
public:
  Holder(Function &function, ControlAnalysis &analysis)
      : fn(function), analysis(analysis), layout(function.getParent()->getDataLayout()),
        libraryImpl(Triple(function.getParent()->getTargetTriple())), library(libraryImpl), assumptions(function),
        dominators(function), loops(dominators), evolution(function, library, assumptions, dominators, loops) {}

  void hold(const std::vector<Instruction *> &writes, Instruction *start, const std::set<const BasicBlock *> &region);

private:
  enum class State { None, Ordinary, Live, Held, Mixed };

  Function &fn;
  ControlAnalysis &analysis;
  const DataLayout &layout;
  TargetLibraryInfoImpl libraryImpl;
  TargetLibraryInfo library;
  AssumptionCache assumptions;
  DominatorTree dominators;
  LoopInfo loops;
  ScalarEvolution evolution;

  // The bytes `pointer` may name for an access of `size` bytes, from `object`'s start: exact for a constant
  // offset, LLVM's bound on the offset's values otherwise.
  std::optional<Range> rangeOf(Value *pointer, uint64_t size, Value *object, bool &exact);
  // `owned`: a local of the function, whose stack bytes never hold qubits: the shadow takes the old value by
  // XOR, holds the local's bytes to the end, and is exchanged into them only where they are measured.
  void span(Value *object, const Range &hull, Instruction *start, const std::set<const BasicBlock *> &region,
            bool owned);
  Function *exchange();
};

std::optional<Range> Holder::rangeOf(Value *pointer, uint64_t size, Value *object, bool &exact) {
  APInt offset(64, 0);
  exact = pointer->stripAndAccumulateConstantOffsets(layout, offset, true) == object;
  if (exact) {
    if (offset.isNegative()) {
      return std::nullopt;
    }
    return Range{offset.getZExtValue(), offset.getZExtValue() + size};
  }
  const SCEV *distance = evolution.getMinusSCEV(evolution.getSCEV(pointer), evolution.getSCEV(object));
  if (isa<SCEVCouldNotCompute>(distance)) {
    return std::nullopt;
  }
  ConstantRange values = evolution.getUnsignedRange(distance);
  if (values.isFullSet() || values.isWrappedSet() ||
      (values.getUnsignedMax() - values.getUnsignedMin()).uge(maxHeldBytes)) {
    return std::nullopt; // more positions than one shadow holds bytes
  }
  return Range{values.getUnsignedMin().getZExtValue(), values.getUnsignedMax().getZExtValue() + size};
}

Function *Holder::exchange() {
  Module &module = *fn.getParent();
  LLVMContext &context = module.getContext();
  Type *pointer = PointerType::get(context, 0);
  FunctionCallee callee = module.getOrInsertFunction(exchangeName, Type::getVoidTy(context), pointer, pointer,
                                                     Type::getInt64Ty(context));
  return cast<Function>(callee.getCallee());
}

void Holder::hold(const std::vector<Instruction *> &writes, Instruction *start,
                  const std::set<const BasicBlock *> &region) {
  // The bytes each object's writes may write.
  std::map<Value *, std::pair<Range, Instruction *>> hulls;
  std::vector<Value *> order;
  for (Instruction *write : writes) {
    auto *store = dyn_cast<StoreInst>(write);
    if (!store) {
      fail("internal: a held write that is not a store");
    }
    Value *object = objectOf(store->getPointerOperand());
    if (!object) {
      notHandled("a store under a qubit into memory whose object the front cannot name (a global variable, a "
                 "pointer argument or a local variable; not built for others)",
                 store);
    }
    bool exact = false;
    std::optional<Range> range =
        rangeOf(store->getPointerOperand(),
                layout.getTypeStoreSize(store->getValueOperand()->getType()).getFixedValue(), object, exact);
    if (!range || (!exact && !region.count(store->getParent()))) {
      notHandled("a store under a qubit into " + objectName(object) +
                     " at a position the front cannot bound to the 8 bytes of one register (rule A over a wider "
                     "region or at an address that depends on a qubit: not built, HANDOFF.md)",
                 store);
    }
    auto [found, fresh] = hulls.try_emplace(object, *range, store);
    if (fresh) {
      order.push_back(object);
    } else {
      found->second.first.begin = std::min(found->second.first.begin, range->begin);
      found->second.first.end = std::max(found->second.first.end, range->end);
    }
  }
  for (Value *object : order) {
    auto [hull, first] = hulls.at(object);
    if (hull.end - hull.begin > maxHeldBytes) {
      notHandled("stores under a qubit into " + describe(object, hull) +
                     ", more than one register holds (8 bytes; route: one shadow register per 8 bytes, not built)",
                 first);
    }
    bool owned = isa<AllocaInst>(object);
    warning("the writes into " + describe(object, hull) + " depend on a qubit: " +
                (owned ? "this local's value lives in qubits from here on (in a register; its stack bytes are not "
                         "used again), entangled with the qubits the writes depend on, until bennett_measure_bytes "
                         "measures them; ordinary code may not read them before then"
                       : "those bytes hold qubits from here on, entangled with the qubits the writes depend on, "
                         "until bennett_measure_bytes measures them; ordinary code that reads them before then "
                         "faults (qubit_memory_access)"),
            first);
    span(object, hull, start, region, owned);
  }
}

void Holder::span(Value *object, const Range &hull, Instruction *start, const std::set<const BasicBlock *> &region,
                  bool owned) {
  LLVMContext &context = fn.getContext();
  Type *word = IntegerType::get(context, unsigned(8 * (hull.end - hull.begin)));
  IRBuilder<> top(&*fn.getEntryBlock().getFirstInsertionPt());
  // An array of the bytes, so that the variable is exactly as wide as they are (an i48 would take 8 bytes).
  AllocaInst *shadow = top.CreateAlloca(ArrayType::get(top.getInt8Ty(), hull.end - hull.begin), nullptr,
                                        (object->hasName() ? object->getName() : "memory") + ".held");
  shadow->setMetadata(qubitMetadata, MDNode::get(context, {}));
  shadow->setMetadata(heldMetadata, MDNode::get(context, MDString::get(context, describe(object, hull))));
  auto addressAt = [&](IRBuilder<> &b) { return b.CreateConstInBoundsGEP1_64(b.getInt8Ty(), object, hull.begin); };

  // The start: the old value, then the exchange that leaves it in the shadow.
  IRBuilder<> b(start);
  Value *address = addressAt(b);
  Value *old = b.CreateLoad(word, address, "held.old");
  Value *after = b.CreateZExt(old, b.getInt64Ty());
  Instruction *borrow = owned ? static_cast<Instruction *>(b.CreateStore(old, shadow))
                              : b.CreateCall(exchange(), {shadow, address, after});
  std::vector<Instruction *> starting;
  for (Value *value : {address, old, after}) {
    if (auto *inst = dyn_cast<Instruction>(value)) {
      starting.push_back(inst);
    }
  }
  starting.push_back(borrow);

  // What an instruction is to this span.
  enum class Role { Other, Start, End, EndWhole, Access, Passes, Return };
  auto role = [&](Instruction &inst, const Range *&touched) -> Role {
    touched = nullptr;
    if (&inst == borrow) {
      return Role::Start;
    }
    if (std::find(starting.begin(), starting.end(), &inst) != starting.end()) {
      return Role::Other;
    }
    if (isa<ReturnInst>(inst)) {
      return Role::Return;
    }
    if (std::optional<Marker> marker = markerCall(&inst); marker && marker->kind == MarkerKind::MeasureBytes) {
      auto &call = cast<CallBase>(inst);
      Value *measured = call.getArgOperand(0);
      const auto *count = dyn_cast<ConstantInt>(call.getArgOperand(1));
      if (objectOf(measured) != object || !count) {
        return objectOf(measured) ? Role::Other : Role::Passes;
      }
      bool exact = false;
      std::optional<Range> range = rangeOf(measured, count->getZExtValue(), object, exact);
      if (!range || !exact) {
        return Role::Passes;
      }
      if (!range->overlaps(hull)) {
        return Role::Other;
      }
      return hull.within(*range) ? Role::EndWhole : Role::End;
    }
    if (Value *pointer = pointerOf(inst); pointer && objectOf(pointer) == object) {
      return Role::Access;
    }
    if (auto *call = dyn_cast<CallBase>(&inst); call && !isExchange(call)) {
      for (Value *arg : call->args()) {
        if (arg->getType()->isPointerTy() && objectOf(arg) == object) {
          return Role::Passes;
        }
      }
    }
    return Role::Other;
  };
  auto overlapping = [&](Instruction &inst) {
    bool exact = false;
    uint64_t size = layout.getTypeStoreSize(accessType(inst)).getFixedValue();
    std::optional<Range> range = rangeOf(pointerOf(inst), size, object, exact);
    return !range || range->overlaps(hull);
  };
  auto merge = [](State a, State b) {
    if (a == State::None || a == b) {
      return b;
    }
    if (b == State::None) {
      return a;
    }
    if (a == State::Live || b == State::Live || a == State::Mixed || b == State::Mixed) {
      return State::Mixed;
    }
    return State::Held; // ordinary on some paths, holding qubits on others: not ordinary
  };

  // The state of the bytes at each block's entry; a conflict (the span would start twice, or end on some paths
  // only, or an access sees a state that differs from path to path) moves the start up the dominator tree.
  for (;;) {
    std::map<const BasicBlock *, State> entry;
    std::vector<BasicBlock *> blocks;
    for (BasicBlock &block : fn) {
      blocks.push_back(&block);
    }
    entry[&fn.getEntryBlock()] = State::Ordinary;
    std::map<const BasicBlock *, State> exitState;
    bool changed = true;
    Instruction *conflict = nullptr;
    auto walk = [&](BasicBlock &block, State state, bool apply) {
      for (Instruction &inst : make_early_inc_range(block)) {
        const Range *touched = nullptr;
        Role kind = role(inst, touched);
        if (kind == Role::Access && !overlapping(inst)) {
          kind = Role::Other;
        }
        if (kind == Role::Other) {
          continue;
        }
        if (state == State::Mixed || (kind == Role::Start && state != State::Ordinary)) {
          conflict = conflict ? conflict : &inst;
          if (!apply) {
            continue;
          }
        }
        switch (kind) {
        case Role::Other:
          break;
        case Role::Start:
          state = State::Live;
          break;
        case Role::Return:
        case Role::End:
        case Role::EndWhole:
          if (owned && kind == Role::End && state == State::Live && apply) {
            notHandled("a measurement of part of " + describe(object, hull) +
                           ", which were written under a qubit (measure all of them: the others would hold qubits "
                           "in the stack frame after it returns)",
                       &inst);
          }
          if (state == State::Live && apply && !(owned && kind == Role::Return)) {
            // A local's stack bytes still hold its old value: the shadow takes it back, a value known here.
            IRBuilder<> end(&inst);
            end.CreateCall(exchange(), {shadow, addressAt(end), owned ? after : end.getInt64(0)});
          }
          if (kind != Role::Return && state == State::Live) {
            state = kind == Role::EndWhole ? State::Ordinary : State::Held;
          }
          break;
        case Role::Passes:
          if (apply && (state == State::Live || state == State::Held)) {
            bool call = !markerCall(&inst);
            notHandled(std::string(call ? "a call that is given memory of " : "a measurement of memory of ") +
                           objectName(object) + " while " + describe(object, hull) + " hold qubits (" +
                           (call ? "measure them first with bennett_measure_bytes)"
                                 : "measure them at their own address, with a constant number of bytes)"),
                       &inst);
          }
          break;
        case Role::Access:
          if (!apply || state == State::Ordinary) {
            break;
          }
          if (state == State::Held) {
            notHandled("ordinary code that touches " + describe(object, hull) +
                           " after they were given qubits (measure them first with bennett_measure_bytes)",
                       &inst);
          }
          {
            Value *pointer = pointerOf(inst);
            bool exact = false;
            uint64_t size = layout.getTypeStoreSize(accessType(inst)).getFixedValue();
            std::optional<Range> range = rangeOf(pointer, size, object, exact);
            if (!range || !range->within(hull)) {
              notHandled("an access of memory that holds qubits in " + describe(object, hull) +
                             " only in part (not built: hold the bytes it touches together)",
                         &inst);
            }
            IRBuilder<> at(&inst);
            Value *redirected = nullptr;
            if (exact) {
              redirected = at.CreateConstInBoundsGEP1_64(at.getInt8Ty(), shadow, range->begin - hull.begin);
            } else {
              if (!region.count(inst.getParent())) {
                notHandled("an access at a position that is not constant into " + describe(object, hull) +
                               ", which hold qubits, outside code under a qubit applied whole (not built)",
                           &inst);
              }
              Value *distance = at.CreateSub(at.CreatePtrToInt(pointer, at.getInt64Ty()),
                                             at.CreatePtrToInt(addressAt(at), at.getInt64Ty()));
              redirected = at.CreateGEP(at.getInt8Ty(), shadow, distance);
            }
            inst.setOperand(isa<LoadInst>(inst) ? 0 : 1, redirected);
          }
          break;
        }
      }
      return state;
    };
    while (changed) {
      changed = false;
      for (BasicBlock *block : blocks) {
        auto found = entry.find(block);
        if (found == entry.end()) {
          continue;
        }
        State out = walk(*block, found->second, false);
        if (exitState[block] != out) {
          exitState[block] = out;
          changed = true;
        }
        for (BasicBlock *succ : successors(block)) {
          State merged = merge(entry.count(succ) ? entry[succ] : State::None, out);
          if (!entry.count(succ) || entry[succ] != merged) {
            entry[succ] = merged;
            changed = true;
          }
        }
      }
    }
    if (!conflict) {
      for (BasicBlock *block : blocks) {
        if (entry.count(block)) {
          walk(*block, entry[block], true);
        }
      }
      return;
    }
    BasicBlock *at = borrow->getParent();
    DomTreeNode *node = dominators.getNode(at);
    if (!node || !node->getIDom()) {
      notHandled("memory written under a qubit (" + describe(object, hull) +
                     ") that would hold qubits on some paths through the function only (it is measured on some "
                     "paths and not on others, or read where it may or may not hold qubits)",
                 conflict);
    }
    Instruction *before = node->getIDom()->getBlock()->getTerminator();
    for (Instruction *inst : starting) {
      inst->moveBefore(before);
    }
  }
}

} // namespace

bool isExchange(const Value *value) {
  const auto *call = dyn_cast_or_null<CallBase>(value);
  const Function *callee = call ? call->getCalledFunction() : nullptr;
  return callee && callee->getName() == exchangeName;
}

void holdWrites(Function &function, const std::vector<Instruction *> &writes, Instruction *start,
                const std::set<const BasicBlock *> &region, ControlAnalysis &analysis) {
  Holder(function, analysis).hold(writes, start, region);
}

bool holdStraightWrite(Function &function, ControlAnalysis &analysis) {
  for (Instruction &inst : instructions(function)) {
    auto *store = dyn_cast<StoreInst>(&inst);
    if (!store || analysis.basedOn(store->getPointerOperand()) || !analysis.tainted(store->getValueOperand()) ||
        analysis.tainted(store->getPointerOperand())) {
      continue;
    }
    holdWrites(function, {store}, store, {}, analysis);
    return true;
  }
  return false;
}

bool finishHeld(Function &function, ControlAnalysis &analysis) {
  const DataLayout &layout = function.getParent()->getDataLayout();
  for (const auto &storage : analysis.storages) {
    if (!storage->held) {
      continue;
    }
    for (Instruction &inst : instructions(function)) {
      Value *pointer = pointerOf(inst);
      if (!pointer || analysis.basedOn(pointer) != storage.get()) {
        continue;
      }
      std::optional<Access> place = analysis.access(pointer);
      if (!place) {
        notHandled("an access at a position that is not constant into " + storage->name +
                       ", outside code under a qubit applied whole (not built)",
                   &inst);
      }
      auto *store = dyn_cast<StoreInst>(&inst);
      unsigned bits = store ? unsigned(layout.getTypeStoreSizeInBits(store->getValueOperand()->getType())) : 0;
      if (!store || (place->offset == 0 && bits == storage->bits)) {
        continue;
      }
      // The whole shadow, with the bytes stored replaced: an overwrite of the shadow that keeps the others.
      IRBuilder<> b(store);
      Type *whole = IntegerType::get(function.getContext(), storage->bits);
      Value *value = store->getValueOperand();
      if (!value->getType()->isIntegerTy()) {
        notHandled("a store of a value that is not an integer into " + storage->name, store);
      }
      APInt mask = APInt::getBitsSet(storage->bits, 8 * place->offset, 8 * place->offset + bits);
      Value *old = b.CreateLoad(whole, storage->alloca, "held.whole");
      Value *placed = b.CreateShl(b.CreateZExt(value, whole), 8 * place->offset);
      Value *updated = b.CreateOr(b.CreateAnd(old, ConstantInt::get(whole, ~mask)), placed);
      b.CreateStore(updated, storage->alloca);
      store->eraseFromParent();
      return true;
    }
  }
  return false;
}

} // namespace bennett
