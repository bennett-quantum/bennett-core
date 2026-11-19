#include "HeldBytes.h"

#include "Aliasing.h"
#include "Diagnostic.h"
#include "Markers.h"
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

// Whether code under a qubit (`region`, entered from `start`'s block) runs one of `writes` on every ordinary
// path through it: whatever the ordinary values, some configuration of the qubits writes. The bytes these writes
// may write hold qubits from before the code on, so where this fails the exchange there runs also on ordinary
// paths on which C writes nothing (docs/DESIGN.md, "Memory written under a qubit"). A block is *sure* when,
// whatever the ordinary values, some configuration of the qubits reaches a write from it before the code ends:
// it writes, or it jumps unconditionally or on an ordinary value to sure blocks only, or on a value chosen by
// qubits alone to at least one sure block. Leaving the code is not sure. A jump back to the head of a loop counts
// as sure for an ordinary choice (staying in the loop forever never reaches code that could read the bytes; it
// leaves by an edge that is checked) and not for a choice by qubits. Each condition chosen by qubits is taken as
// free, as the text reads: `if (q) if (!q) g = q;` counts as a write.
bool writesOnEveryOrdinaryPath(const std::vector<Instruction *> &writes, const Instruction &start,
                                const std::set<const BasicBlock *> &region, const ControlAnalysis &analysis) {
  SmallVector<std::pair<const BasicBlock *, const BasicBlock *>, 8> backList;
  FindFunctionBackedges(*start.getFunction(), backList);
  std::set<std::pair<const BasicBlock *, const BasicBlock *>> back(backList.begin(), backList.end());
  std::set<const BasicBlock *> sure;
  for (const Instruction *write : writes) {
    sure.insert(write->getParent());
  }
  for (bool changed = true; changed;) {
    changed = false;
    for (const BasicBlock *block : region) {
      if (sure.count(block)) {
        continue;
      }
      const Instruction *term = block->getTerminator();
      const auto *branch = dyn_cast<BranchInst>(term);
      const auto *choice = dyn_cast<SwitchInst>(term);
      if (!branch && !choice) {
        continue;
      }
      const Value *condition = branch && branch->isConditional() ? branch->getCondition()
                               : choice                          ? choice->getCondition()
                                                                 : nullptr;
      bool chosen = condition && chosenByQubits(condition, analysis);
      bool any = false;
      bool all = true;
      for (const BasicBlock *succ : successors(block)) {
        bool good = region.count(succ) && sure.count(succ);
        bool again = back.count({block, succ}) != 0;
        any |= good && !again;
        all &= good || again;
      }
      if (chosen ? any : all) {
        sure.insert(block);
        changed = true;
      }
    }
  }
  const BasicBlock *entry = start.getParent()->getSingleSuccessor();
  return entry && region.count(entry) && sure.count(entry);
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
  // `everyPath`: the writes run on every ordinary path from `start` (writesOnEveryOrdinaryPath); otherwise the
  // span is refused where the bytes may still hold qubits when the function returns.
  void span(Value *object, const Range &hull, Instruction *start, const std::set<const BasicBlock *> &region,
            bool owned, bool everyPath);
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
    span(object, hull, start, region, owned, region.empty() || writesOnEveryOrdinaryPath(own, *start, region, analysis));
  }
}

void Holder::span(Value *object, const Range &hull, Instruction *start, const std::set<const BasicBlock *> &region,
                  bool owned, bool everyPath) {
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
          // Where the exchange may have run on a path on which C writes nothing, the bytes must not leave the
          // function holding qubits: the caller may read them ordinarily, where C reads the old value.
          if (apply && !owned && !everyPath && state.holdsQubits()) {
            notHandled("memory written under a qubit (" + describe(object, hull) +
                           ") whose exchange runs also on ordinary paths on which C writes nothing there (an ordinary "
                           "condition skips the write after the exchange), and which may still hold qubits when the "
                           "function returns, where the caller can read them (measure them before returning; route: "
                           "one exchange on each path that writes, not built)",
                       &inst);
          }
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
        fail("cannot make a block after code under a qubit to end a span of held bytes");
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
    Instruction *before = node->getIDom()->getBlock()->getTerminator();
    // Above an ordinary condition that skips the write (the span's original start does not post-dominate the new
    // place) the exchange runs also where C writes nothing: the bytes are then refused at a return that may see
    // them hold qubits. Inside the function every access while the shadow holds them reads the shadow, which
    // holds their old value there.
    everyPath = everyPath && PostDominatorTree(fn).dominates(start->getParent(), before->getParent());
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
