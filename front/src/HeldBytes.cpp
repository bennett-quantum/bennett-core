#include "HeldBytes.h"

#include "Classify.h"

#include "Aliasing.h"
#include "Diagnostic.h"
#include "Markers.h"
#include "Outline.h"
#include "Taint.h"

#include <llvm/ADT/SmallPtrSet.h>
#include <llvm/ADT/SmallVector.h>
#include <llvm/Analysis/AssumptionCache.h>
#include <llvm/Analysis/CFG.h>
#include <llvm/Analysis/LoopInfo.h>
#include <llvm/Analysis/PostDominators.h>
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
#include <llvm/Transforms/Utils/BasicBlockUtils.h>

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

// Whether a branch on `condition` may take either edge for every ordinary input: it depends on a qubit and on
// nothing ordinary, so no ordinary value can rule an edge out. A condition with an ordinary operand (`n && q`,
// which LLVM makes of `if (q) if (n)`, or `q + n > 3`) may take one edge only for some ordinary inputs.
bool chosenByQubits(const Value *condition, const ControlAnalysis &analysis) {
  std::set<const Value *> seen;
  std::vector<const Value *> work{condition};
  while (!work.empty()) {
    const Value *value = work.back();
    work.pop_back();
    if (isa<Constant>(value) || !seen.insert(value).second) {
      continue;
    }
    if (!analysis.tainted(value)) {
      return false;
    }
    if (analysis.isQuantumLoad(value)) {
      continue;
    }
    const auto *inst = dyn_cast<Instruction>(value);
    if (!inst || inst->mayReadOrWriteMemory()) {
      return false;
    }
    for (const Use &operand : inst->operands()) {
      work.push_back(operand.get());
    }
  }
  return true;
}

class Holder {
public:
  Holder(Function &function, ControlAnalysis &analysis)
      : fn(function), analysis(analysis), layout(function.getParent()->getDataLayout()),
        libraryImpl(Triple(function.getParent()->getTargetTriple())), library(libraryImpl), assumptions(function),
        dominators(function), loops(dominators), evolution(function, library, assumptions, dominators, loops),
        aliasing(*function.getParent()) {}

  void hold(const std::vector<Instruction *> &writes, Instruction *start, const std::set<const BasicBlock *> &region);

private:
  Function &fn;
  ControlAnalysis &analysis;
  const DataLayout &layout;
  TargetLibraryInfoImpl libraryImpl;
  TargetLibraryInfo library;
  AssumptionCache assumptions;
  DominatorTree dominators;
  LoopInfo loops;
  ScalarEvolution evolution;
  Aliasing aliasing;

  // The bytes `pointer` may name for an access of `size` bytes, from `object`'s start: exact for a constant
  // offset, LLVM's bound on the offset's values otherwise.
  std::optional<Range> rangeOf(Value *pointer, uint64_t size, Value *object, bool &exact);
  // `owned`: a local of the function, whose stack bytes never hold qubits: the shadow takes the old value by
  // XOR, holds the local's bytes to the end, and is exchanged into them only where they are measured.
  // `writes`: the stores of values that depend on a qubit into `object` that made the front hold it.
  void span(Value *object, const Range &hull, Instruction *start, const std::set<const BasicBlock *> &region,
            bool owned, const std::vector<Instruction *> &writes);
  // Whether, from `from` on, every ordinary path writes each byte of `hull` in `object` with a store whose value,
  // or whether it runs, depends on a qubit (below).
  bool writesEveryByte(const Instruction &from, Value *object, const Range &hull,
                       const std::set<const BasicBlock *> &region, const std::vector<Instruction *> &writes);
  // Whether `store` writes a value that depends on a qubit, or runs only where a qubit says: in `region`, among
  // `writes`, or in a block that depends directly on a conditional jump on a value that depends on a qubit (the
  // block post-dominates one successor of the jump and not the jump's own block). An under-approximation.
  bool qubitWrite(const StoreInst &store, const std::set<const BasicBlock *> &region,
                  const std::vector<Instruction *> &writes);
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
  // One shadow per object: two objects whose bytes may be the same would take the same bytes twice.
  for (size_t i = 0; i < order.size(); ++i) {
    for (size_t j = i + 1; j < order.size(); ++j) {
      if (aliasing.mayShare(fn, order[i], order[j])) {
        notHandled("stores under a qubit through " + objectName(order[i]) + " and " + objectName(order[j]) +
                       ", which may reach the same bytes (the front keeps one shadow per object and cannot tell "
                       "them apart; route: a static function whose calls pass distinct objects or the same pointer "
                       "for both, `restrict`, or a check of the addresses at run time: not built)",
                   hulls.at(order[j]).second);
      }
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
    std::vector<Instruction *> own;
    for (Instruction *write : writes) {
      if (objectOf(cast<StoreInst>(write)->getPointerOperand()) == object) {
        own.push_back(write);
      }
    }
    span(object, hull, start, region, owned, own);
  }
}

// The exchange that starts a span runs only where it must: on every ordinary path from it, each byte it takes is
// written by a store that makes it hold qubits (the language's rule; docs/DESIGN.md, "Memory written under a
// qubit"). This is decided per byte and as an allow-list. A block is *sure* for a byte when, whatever the ordinary
// values, some configuration of the qubits writes the byte from it before the function returns: it contains such a
// store of exactly that byte (a constant position covering it), or it jumps unconditionally or on an ordinary
// value only to sure blocks, or on a value chosen by qubits alone (each such condition read as free, as the text
// reads) to at least one. Nothing is credited to a jump back into a loop: a pass that skips the store proves
// nothing about the next. A return is not sure.
bool Holder::writesEveryByte(const Instruction &from, Value *object, const Range &hull,
                             const std::set<const BasicBlock *> &region, const std::vector<Instruction *> &writes) {
  auto writesByte = [&](const Instruction &inst, uint64_t byte) {
    auto *store = dyn_cast<StoreInst>(const_cast<Instruction *>(&inst));
    if (!store || objectOf(store->getPointerOperand()) != object) {
      return false;
    }
    bool byQubit = qubitWrite(*store, region, writes);
    bool exact = false;
    std::optional<Range> range = rangeOf(store->getPointerOperand(),
                                         layout.getTypeStoreSize(store->getValueOperand()->getType()).getFixedValue(),
                                         object, exact);
    return byQubit && exact && range && range->begin <= byte && byte < range->end;
  };
  for (uint64_t byte = hull.begin; byte < hull.end; ++byte) {
    std::set<const BasicBlock *> sure;
    for (const BasicBlock &block : fn) {
      if (any_of(block, [&](const Instruction &inst) { return writesByte(inst, byte); })) {
        sure.insert(&block);
      }
    }
    auto follows = [&](const BasicBlock &block) {
      const Instruction *term = block.getTerminator();
      const auto *branch = dyn_cast<BranchInst>(term);
      const auto *choice = dyn_cast<SwitchInst>(term);
      if (!branch && !choice) {
        return false; // a return, or a trap
      }
      const Value *condition = branch && branch->isConditional() ? branch->getCondition()
                               : choice                          ? choice->getCondition()
                                                                 : nullptr;
      bool chosen = condition && chosenByQubits(condition, analysis);
      bool any = false;
      bool all = true;
      for (const BasicBlock *succ : successors(&block)) {
        any |= sure.count(succ) != 0;
        all &= sure.count(succ) != 0;
      }
      return chosen ? any : all;
    };
    for (bool changed = true; changed;) {
      changed = false;
      for (const BasicBlock &block : fn) {
        if (!sure.count(&block) && follows(block)) {
          sure.insert(&block);
          changed = true;
        }
      }
    }
    bool after = false;
    bool written = false;
    for (const Instruction &inst : *from.getParent()) {
      written |= after && writesByte(inst, byte);
      after |= &inst == &from;
    }
    if (!written && !follows(*from.getParent())) {
      return false;
    }
  }
  return true;
}

bool Holder::qubitWrite(const StoreInst &store, const std::set<const BasicBlock *> &region,
                        const std::vector<Instruction *> &writes) {
  if (analysis.tainted(store.getValueOperand()) || region.count(store.getParent()) ||
      std::find(writes.begin(), writes.end(), &store) != writes.end()) {
    return true;
  }
  PostDominatorTree after(fn);
  const BasicBlock *block = store.getParent();
  for (const BasicBlock &jumpBlock : fn) {
    const auto *jump = dyn_cast<BranchInst>(jumpBlock.getTerminator());
    if (!jump || !jump->isConditional() || !analysis.tainted(jump->getCondition()) ||
        after.properlyDominates(block, &jumpBlock)) {
      continue;
    }
    for (const BasicBlock *succ : successors(&jumpBlock)) {
      if (after.dominates(block, succ)) {
        return true;
      }
    }
  }
  return false;
}

void Holder::span(Value *object, const Range &hull, Instruction *start, const std::set<const BasicBlock *> &region,
                  bool owned, const std::vector<Instruction *> &writes) {
  bool everyPath = true; // decided once the paths agree, before the accesses are redirected
  // Where every path writes the bytes: what runs after the exchange and before the first store that makes all
  // of them hold qubits, on some path (an ordinary access there would see the shadow where C sees memory).
  std::set<const Instruction *> beforeWrite;
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

  // The hull's bytes as a mask, bit i for the byte at hull.begin + i; `range`'s bytes within the hull.
  const unsigned count = unsigned(hull.end - hull.begin);
  const uint8_t all = uint8_t((1u << count) - 1);
  auto bytesOf = [&](const Range &range) {
    uint8_t mask = 0;
    for (uint64_t at = std::max(range.begin, hull.begin); at < std::min(range.end, hull.end); ++at) {
      mask |= uint8_t(1u << (at - hull.begin));
    }
    return mask;
  };
  // The exchanges this span put on edges where a path on which the shadow holds the bytes joins one on which the
  // span has ended.
  std::set<Instruction *> edgeEnds;

  // What an instruction is to this span. `touched`: the bytes a measurement or an access names, when known.
  enum class Role { Other, Start, End, EdgeEnd, Access, Shares, Passes, Shadowed, Return };
  auto role = [&](Instruction &inst, std::optional<Range> &touched) -> Role {
    touched.reset();
    if (&inst == borrow) {
      return Role::Start;
    }
    if (std::find(starting.begin(), starting.end(), &inst) != starting.end()) {
      return Role::Other;
    }
    if (edgeEnds.count(&inst)) {
      return Role::EdgeEnd;
    }
    if (isa<ReturnInst>(inst)) {
      return Role::Return;
    }
    if (std::optional<Marker> marker = markerCall(&inst); marker && marker->kind == MarkerKind::MeasureBytes) {
      auto &call = cast<CallBase>(inst);
      Value *measured = call.getArgOperand(0);
      Value *measuredObject = objectOf(measured);
      if (measuredObject != object) {
        return measuredObject && !aliasing.mayShare(fn, measured, object) ? Role::Other : Role::Passes;
      }
      const auto *bytes = dyn_cast<ConstantInt>(call.getArgOperand(1));
      bool exact = false;
      std::optional<Range> range = bytes ? rangeOf(measured, bytes->getZExtValue(), object, exact) : std::nullopt;
      if (!range || !exact) {
        return Role::Passes;
      }
      if (!range->overlaps(hull)) {
        return Role::Other;
      }
      touched = range;
      return Role::End;
    }
    if (Value *pointer = pointerOf(inst)) {
      if (objectOf(pointer) == object) {
        bool exact = false;
        touched = rangeOf(pointer, layout.getTypeStoreSize(accessType(inst)).getFixedValue(), object, exact);
        return touched && !touched->overlaps(hull) ? Role::Other : Role::Access;
      }
      return aliasing.mayShare(fn, pointer, object) ? Role::Shares : Role::Other;
    }
    if (auto *rmw = dyn_cast<AtomicRMWInst>(&inst)) {
      return aliasing.mayShare(fn, rmw->getPointerOperand(), object) ? Role::Shares : Role::Other;
    }
    if (auto *exchanged = dyn_cast<AtomicCmpXchgInst>(&inst)) {
      return aliasing.mayShare(fn, exchanged->getPointerOperand(), object) ? Role::Shares : Role::Other;
    }
    if (isExchange(&inst)) {
      // Another shadow's exchange (this span's own are Start, EdgeEnd, or not walked again once inserted): it
      // must not take bytes this shadow may hold.
      auto &call = cast<CallBase>(inst);
      auto *other = dyn_cast<AllocaInst>(call.getArgOperand(0));
      Value *address = call.getArgOperand(1);
      if (other == shadow) {
        return Role::Other;
      }
      auto *bytes = other ? dyn_cast<ArrayType>(other->getAllocatedType()) : nullptr;
      if (objectOf(address) == object && bytes) {
        bool exact = false;
        std::optional<Range> range = rangeOf(address, bytes->getNumElements(), object, exact);
        if (exact && range && !range->overlaps(hull)) {
          return Role::Other;
        }
        return Role::Shadowed;
      }
      return aliasing.mayShare(fn, address, object) ? Role::Shadowed : Role::Other;
    }
    if (auto *call = dyn_cast<CallBase>(&inst); call && !markerCall(call)) {
      return aliasing.mayTouch(fn, *call, object) ? Role::Passes : Role::Other;
    }
    return Role::Other;
  };

  // The bytes at a point: not taken yet (Before), held in the shadow (Live), given back to memory with `held` the
  // bytes that still hold qubits there (Done; 0: all ordinary again), or different from path to path (Mixed).
  struct State {
    enum Kind { None, Before, Live, Done, Mixed } kind = None;
    uint8_t held = 0;
    bool operator==(const State &other) const { return kind == other.kind && held == other.held; }
    bool operator!=(const State &other) const { return !(*this == other); }
    bool holdsQubits() const { return kind == Live || (kind == Done && held != 0); }
  };
  auto merge = [](State a, State b) {
    if (a.kind == State::None || a == b) {
      return b;
    }
    if (b.kind == State::None) {
      return a;
    }
    if (a.kind == State::Live || b.kind == State::Live || a.kind == State::Mixed || b.kind == State::Mixed) {
      return State{State::Mixed, 0};
    }
    return State{State::Done, uint8_t(a.held | b.held)}; // ordinary on some paths, holding qubits on others
  };
  auto refuseHeld = [&](Instruction &inst, Role kind) {
    if (kind == Role::Shadowed) {
      notHandled("memory written under a qubit, " + describe(object, hull) +
                     ", that another write under a qubit holds again while they hold qubits (the front keeps one "
                     "shadow per object; it takes the earlier write first when that write comes before the code "
                     "under a qubit on every path and outside a loop; route for other orders: one shadow from the "
                     "earliest write, not built)",
                 &inst);
    }
    if (kind == Role::Shares) {
      notHandled("an access through a pointer that may reach " + describe(object, hull) +
                     " while they hold qubits (the front keeps one shadow per object and cannot tell these pointers "
                     "apart; route: a static function whose calls pass distinct objects, `restrict`, or a check of "
                     "the addresses at run time: not built)",
                 &inst);
    }
    bool call = !markerCall(&inst);
    notHandled(std::string(call ? "a call that may read or write memory of " : "a measurement of memory of ") +
                   objectName(object) + " while " + describe(object, hull) + " hold qubits (" +
                   (call ? "measure them first with bennett_measure_bytes)"
                         : "measure them at their own address, with a constant number of bytes)"),
               &inst);
  };

  // Whether everything `load` computes ends in stores of values that depend on a qubit into these same bytes:
  // the old value of an update the front predicated (`x = reached ? v : x`). On a path on which C does not write
  // the bytes the shadow holds their old value, and the store puts it back.
  auto feedsOnlyQubitStoresHere = [&](LoadInst *load) {
    std::set<const Value *> seen;
    std::vector<const Value *> work{load};
    while (!work.empty()) {
      const Value *value = work.back();
      work.pop_back();
      if (!seen.insert(value).second) {
        continue;
      }
      for (const User *user : value->users()) {
        if (const auto *store = dyn_cast<StoreInst>(user)) {
          if (store->getValueOperand() != value || !analysis.tainted(value) ||
              objectOf(const_cast<Value *>(store->getPointerOperand())) != object) {
            return false;
          }
          continue;
        }
        const auto *inst = dyn_cast<Instruction>(user);
        if (!inst || inst->mayHaveSideEffects() || isa<PHINode>(inst) || inst->isTerminator() ||
            isa<CallBase>(inst) || isa<LoadInst>(inst)) {
          return false;
        }
        work.push_back(inst);
      }
    }
    return true;
  };

  // Whether nothing from the start of `from` on touches the bytes but a measurement of them or a return.
  auto quietFrom = [&](BasicBlock &from) {
    std::set<const BasicBlock *> seen{&from};
    std::vector<BasicBlock *> work{&from};
    while (!work.empty()) {
      BasicBlock *block = work.back();
      work.pop_back();
      for (Instruction &inst : *block) {
        std::optional<Range> touched;
        Role kind = role(inst, touched);
        if (kind != Role::Other && kind != Role::End && kind != Role::Return && kind != Role::EdgeEnd) {
          return false;
        }
      }
      for (BasicBlock *succ : successors(block)) {
        if (seen.insert(succ).second) {
          work.push_back(succ);
        }
      }
    }
    return true;
  };

  // The state of the bytes at each block's entry. A conflict (the span would start twice, or an access sees a
  // state that differs from path to path) is settled by an exchange on each edge that leaves the shadow holding
  // the bytes where it joins paths on which the span has ended, or else moves the start up the dominator tree.
  for (;;) {
    std::map<const BasicBlock *, State> entry;
    std::vector<BasicBlock *> blocks;
    for (BasicBlock &block : fn) {
      blocks.push_back(&block);
    }
    entry[&fn.getEntryBlock()] = State{State::Before, 0};
    std::map<const BasicBlock *, State> exitState;
    bool changed = true;
    Instruction *conflict = nullptr;
    auto walk = [&](BasicBlock &block, State state, bool apply) {
      for (Instruction &inst : make_early_inc_range(block)) {
        std::optional<Range> touched;
        Role kind = role(inst, touched);
        if (kind == Role::Other) {
          continue;
        }
        bool startable = state.kind == State::Before || (state.kind == State::Done && state.held == 0);
        if (state.kind == State::Mixed || (kind == Role::Start && !startable) ||
            (kind == Role::EdgeEnd && state.kind != State::Live)) {
          conflict = conflict ? conflict : &inst;
          if (!apply) {
            continue;
          }
        }
        switch (kind) {
        case Role::Other:
          break;
        case Role::Start:
          state = State{State::Live, 0};
          break;
        case Role::EdgeEnd:
          state = State{State::Done, all};
          break;
        case Role::Return:
          if (state.kind == State::Live && apply && !owned) {
            IRBuilder<> end(&inst);
            end.CreateCall(exchange(), {shadow, addressAt(end), end.getInt64(0)});
          }
          break;
        case Role::End: {
          uint8_t measured = bytesOf(*touched);
          if (state.kind == State::Live) {
            if (owned && measured != all && apply) {
              notHandled("a measurement of part of " + describe(object, hull) +
                             ", which were written under a qubit (measure all of them: the others would hold "
                             "qubits in the stack frame after it returns)",
                         &inst);
            }
            if (apply) {
              // A local's stack bytes still hold its old value: the shadow takes it back, a value known here.
              IRBuilder<> end(&inst);
              end.CreateCall(exchange(), {shadow, addressAt(end), owned ? after : end.getInt64(0)});
            }
            state = State{State::Done, uint8_t(all & ~measured)};
          } else if (state.kind == State::Done) {
            state.held &= uint8_t(~measured);
          }
          break;
        }
        case Role::Passes:
        case Role::Shares:
        case Role::Shadowed:
          if (apply && state.holdsQubits()) {
            refuseHeld(inst, kind);
          }
          break;
        case Role::Access:
          if (!apply || state.kind == State::Before) {
            break;
          }
          // Every path from the exchange writes the bytes (rule 2, below); an ordinary store before that write only
          // sets the shadow, which the write settles, while an ordinary read there would see the shadow where C sees
          // memory. Allowed: code under a qubit, stores that make the bytes hold qubits, and reads that only feed
          // such a store back into them.
          if (!owned && state.kind == State::Live && beforeWrite.count(&inst) && isa<LoadInst>(inst) &&
              !region.count(inst.getParent()) && !feedsOnlyQubitStoresHere(cast<LoadInst>(&inst))) {
            notHandled("an access of " + describe(object, hull) +
                           " after their exchange and before the write that makes them hold qubits on this path: it "
                           "would read their shadow where C reads ordinary memory (route: start the exchange at the "
                           "write, not built)",
                       &inst);
          }
          if (state.kind == State::Done) {
            if (!touched || (bytesOf(*touched) & state.held) != 0) {
              notHandled("ordinary code that touches " + describe(object, hull) +
                             " after they were given qubits (measure them first with bennett_measure_bytes)",
                         &inst);
            }
            break; // bytes measured since: ordinary memory again
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
            declassify(inst); // an access of the shadow, a qubit variable, from now on
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
          State merged = merge(entry.count(succ) ? entry[succ] : State{}, out);
          if (!entry.count(succ) || entry[succ] != merged) {
            entry[succ] = merged;
            changed = true;
          }
        }
      }
    }
    if (!conflict) {
      // Rule 2 of the placement, which denies by default: every ordinary path from the exchange writes every byte
      // it takes; a span from which some path does not is refused by name.
      everyPath = owned || writesEveryByte(*borrow, object, hull, region, writes);
      if (!everyPath) {
        notHandled("memory written under a qubit (" + describe(object, hull) +
                       ") from whose exchange some ordinary path does not write every byte (an ordinary condition, "
                       "a loop over an ordinary count or a position that is not constant may skip a byte; route: "
                       "writes at a position that is not constant proved to cover each byte, or a measurement of "
                       "the bytes before the return with nothing in between, not built; docs/DESIGN.md, \"Not "
                       "yet\")",
                   borrow);
      }
      if (everyPath && !owned) {
        auto writesAll = [&](Instruction &inst) {
          auto *store = dyn_cast<StoreInst>(&inst);
          if (!store || objectOf(store->getPointerOperand()) != object || !qubitWrite(*store, region, writes)) {
            return false;
          }
          bool exact = false;
          std::optional<Range> range = rangeOf(
              store->getPointerOperand(), layout.getTypeStoreSize(store->getValueOperand()->getType()).getFixedValue(),
              object, exact);
          return exact && range && range->begin <= hull.begin && hull.end <= range->end;
        };
        std::set<const BasicBlock *> visited;
        std::vector<BasicBlock *> work;
        bool stopped = false;
        for (Instruction *inst = borrow->getNextNode(); inst && !stopped; inst = inst->getNextNode()) {
          stopped = writesAll(*inst);
          if (!stopped) {
            beforeWrite.insert(inst);
          }
        }
        // A jump on a value chosen by qubits alone, one of whose arms writes the bytes before the arms meet again,
        // writes them on this ordinary path as the text reads: the search goes on from where the arms meet.
        PostDominatorTree after(fn);
        auto armsWrite = [&](BasicBlock &fork, BasicBlock *&meet) {
          const auto *jump = dyn_cast<BranchInst>(fork.getTerminator());
          DomTreeNode *node = after.getNode(&fork);
          if (!jump || !jump->isConditional() || !chosenByQubits(jump->getCondition(), analysis) || !node ||
              !node->getIDom() || !node->getIDom()->getBlock()) {
            return false;
          }
          meet = node->getIDom()->getBlock();
          std::set<const BasicBlock *> seen{meet};
          std::vector<BasicBlock *> arms(succ_begin(&fork), succ_end(&fork));
          while (!arms.empty()) {
            BasicBlock *arm = arms.back();
            arms.pop_back();
            if (!seen.insert(arm).second) {
              continue;
            }
            if (any_of(*arm, [&](Instruction &inst) { return writesAll(inst); })) {
              return true;
            }
            arms.insert(arms.end(), succ_begin(arm), succ_end(arm));
          }
          return false;
        };
        if (BasicBlock *meet = nullptr; !stopped && !armsWrite(*borrow->getParent(), meet)) {
          work.assign(succ_begin(borrow->getParent()), succ_end(borrow->getParent()));
        }
        while (!work.empty()) {
          BasicBlock *block = work.back();
          work.pop_back();
          if (!visited.insert(block).second) {
            continue;
          }
          bool written = false;
          for (Instruction &inst : *block) {
            if ((written = writesAll(inst))) {
              break;
            }
            beforeWrite.insert(&inst);
          }
          BasicBlock *meet = nullptr;
          if (!written && armsWrite(*block, meet)) {
            continue; // written where the arms meet: nothing after counts
          }
          if (!written) {
            work.insert(work.end(), succ_begin(block), succ_end(block));
          }
        }
      }
      for (BasicBlock *block : blocks) {
        if (entry.count(block)) {
          walk(*block, entry[block], true);
        }
      }
      return;
    }
    // Paths on which the shadow still holds the bytes, joining paths on which the span has ended or never started,
    // outside code applied whole: the span ends on those edges too (the exchange, as before a return). Edges that
    // leave code applied whole end it in a block of their own after that code.
    std::vector<std::pair<BasicBlock *, BasicBlock *>> edges;
    std::vector<std::pair<BasicBlock *, SmallVector<BasicBlock *, 4>>> exits;
    for (BasicBlock *block : blocks) {
      if (owned || region.count(block) || !entry.count(block) || entry[block].kind != State::Mixed) {
        continue;
      }
      bool live = false;
      bool done = false;
      bool other = false;
      SmallVector<BasicBlock *, 4> fromRegion;
      for (BasicBlock *pred : predecessors(block)) {
        auto found = exitState.find(pred);
        if (found == exitState.end()) {
          continue;
        }
        // A path on which the span never started (Before) gives no bytes of its own, as one on which it ended,
        // when nothing after the join touches the bytes again but a measurement (else the start moves up, below).
        bool ended = found->second.kind == State::Done || (found->second.kind == State::Before && quietFrom(*block));
        bool held = found->second.kind == State::Live;
        live |= held;
        done |= ended;
        other |= (!held && !ended) || (region.count(pred) && !held);
        if (held && region.count(pred) && !is_contained(fromRegion, pred)) {
          fromRegion.push_back(pred);
        }
      }
      if (live && done && !other) {
        if (!fromRegion.empty()) {
          exits.push_back({block, fromRegion});
        }
        for (BasicBlock *pred : predecessors(block)) {
          if (exitState.count(pred) && exitState[pred].kind == State::Live && !region.count(pred)) {
            edges.push_back({pred, block});
          }
        }
      }
    }
    for (auto &[to, preds] : exits) {
      BasicBlock *after = SplitBlockPredecessors(to, preds, ".held.end", &dominators, &loops);
      if (!after) {
        notHandled("memory written under a qubit (" + describe(object, hull) +
                       ") held by code under a qubit whose exits LLVM cannot gather into a block of their own, to "
                       "end the span there (route: one exchange on each path that writes, not built)",
                   to->getFirstNonPHI());
      }
      IRBuilder<> end(after->getTerminator());
      edgeEnds.insert(end.CreateCall(exchange(), {shadow, addressAt(end), end.getInt64(0)}));
    }
    if (!exits.empty()) {
      continue;
    }
    if (!edges.empty()) {
      for (auto [from, to] : edges) {
        BasicBlock *at = from->getSingleSuccessor() == to ? from : SplitEdge(from, to, &dominators, &loops);
        IRBuilder<> end(at->getTerminator());
        edgeEnds.insert(end.CreateCall(exchange(), {shadow, addressAt(end), end.getInt64(0)}));
      }
      continue;
    }
    BasicBlock *at = borrow->getParent();
    DomTreeNode *node = dominators.getNode(at);
    if (!node || !node->getIDom()) {
      notHandled("memory written under a qubit (" + describe(object, hull) +
                     ") that would hold qubits on some paths through the function only (it is measured on some "
                     "paths and not on others, or read where it may or may not hold qubits)",
                 conflict);
    }
    // Rule 1 of the placement, which denies by default: the exchange stands at the store that needs it (or before
    // the code under a qubit that contains it) and is never moved earlier.
    notHandled("memory written under a qubit (" + describe(object, hull) + ") whose exchange would have to move "
                   "earlier than the store that needs it for the paths to agree where they meet (route: a store of "
                   "every byte on every path after the condition, or a measurement of the bytes before the return, "
                   "each with its proof, not built; docs/DESIGN.md, \"Not yet\")",
               conflict);
  }
}

} // namespace

bool isExchange(const Value *value) {
  const auto *call = dyn_cast_or_null<CallBase>(value);
  const Function *callee = call ? call->getCalledFunction() : nullptr;
  return callee && callee->getName() == exchangeName;
}

void keepOrdinaryLoads(std::vector<LoadInst *> &loads, const Instruction &at,
                       const std::set<const Function *> &control) {
  // Start with globals, which have ordinary bytes when the device starts and whose address is independent of
  // the frame. For other pointers the bytes could come from an earlier call or from a reused frame.
  const DataLayout &layout = at.getModule()->getDataLayout();
  erase_if(loads, [&](const LoadInst *load) {
    int64_t offset;
    const auto *global = dyn_cast<GlobalVariable>(
        GetPointerBaseWithConstantOffset(load->getPointerOperand(), offset, layout));
    if (!global || offset < 0 || !global->getValueType()->isSized()) {
      return true;
    }
    TypeSize size = layout.getTypeAllocSize(global->getValueType());
    TypeSize width = layout.getTypeStoreSize(load->getType());
    return size.isScalable() || width.isScalable() || uint64_t(offset) > size.getFixedValue() ||
           width.getFixedValue() > size.getFixedValue() - uint64_t(offset);
  });
  if (loads.empty()) {
    return;
  }
  Aliasing aliasing(*at.getModule());
  SmallVector<const Instruction *, 8> work{&at};
  SmallPtrSet<const Instruction *, 32> seen;
  while (!work.empty() && !loads.empty()) {
    const Instruction *next = work.pop_back_val();
    if (!seen.insert(next).second) {
      continue;
    }
    const Function &function = *next->getFunction();
    const BasicBlock &block = *next->getParent();
    if (const Instruction *previous = next->getPrevNode()) {
      if (const auto *call = dyn_cast<CallBase>(previous)) {
        if (call->hasFnAttr(Attribute::ReturnsTwice)) {
          // A longjmp may resume here from code the incoming edges do not describe.
          loads.clear();
        } else if (isExchange(call) || control.count(call->getCalledFunction())) {
          erase_if(loads, [&](const LoadInst *load) {
            return aliasing.mayTouch(function, *call, load->getPointerOperand());
          });
        }
      } else if (const auto *store = dyn_cast<StoreInst>(previous)) {
        erase_if(loads, [&](const LoadInst *load) {
          return aliasing.mayShare(function, store->getPointerOperand(), load->getPointerOperand());
        });
      }
      work.push_back(previous);
    } else if (&block == &function.getEntryBlock()) {
      // Control functions can only be called directly. Follow the callers as well as paths within this call.
      for (const User *user : function.users()) {
        work.push_back(cast<CallBase>(user));
      }
    } else {
      for (const BasicBlock *predecessor : predecessors(&block)) {
        work.push_back(predecessor->getTerminator());
      }
    }
  }
}

void holdWrites(Function &function, const std::vector<Instruction *> &writes, Instruction *start,
                const std::set<const BasicBlock *> &region, ControlAnalysis &analysis) {
  mergeEqualArguments(function);
  Holder(function, analysis).hold(writes, start, region);
}

bool holdStraightWrite(Function &function, ControlAnalysis &analysis, const Instruction *before) {
  for (Instruction &inst : instructions(function)) {
    auto *store = dyn_cast<StoreInst>(&inst);
    if (!store || analysis.basedOn(store->getPointerOperand()) || !analysis.tainted(store->getValueOperand()) ||
        analysis.tainted(store->getPointerOperand())) {
      continue;
    }
    if (before && (!analysis.dominators.dominates(store, before) || analysis.loops.getLoopFor(store->getParent()))) {
      continue; // it may run after `before`, or be controlled by it
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
