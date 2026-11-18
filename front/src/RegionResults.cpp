#include "RegionResults.h"

#include "CallerPosition.h"
#include "Diagnostic.h"
#include "Markers.h"
#include "Normalise.h"
#include "Taint.h"

#include <llvm/Analysis/ValueTracking.h>
#include <llvm/IR/Constants.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>
#include <llvm/IR/Verifier.h>
#include <llvm/Support/KnownBits.h>
#include <llvm/Support/raw_ostream.h>
#include <llvm/Transforms/Utils/BasicBlockUtils.h>
#include <llvm/Transforms/Utils/Cloning.h>
#include <llvm/Transforms/Utils/Local.h>
#include <llvm/Transforms/Utils/PromoteMemToReg.h>
#include <llvm/Transforms/Utils/ValueMapper.h>

#include <map>
#include <set>

using namespace llvm;

namespace bennett {

namespace {

// The widest result an applied function returns: a device register holds at most 64 bits.
constexpr unsigned maxResultBits = 64;

// A qubit variable the region writes, and the value it starts from in the private run.
struct Written {
  const Storage *storage;
  Value *known = nullptr; // its known value before the region; null: accumulated, starting at 0
  unsigned offset = 0;    // its field in the result word
  // A store where the region ends, of a value the region computes (`moved`); null: the region's own stores.
  StoreInst *moved = nullptr;
  // When the moved store's value is a phi of the region's exit: its edges from the region, taken before the
  // exit block is split.
  std::vector<std::pair<BasicBlock *, Value *>> incoming{};
  // Read and overwritten in a loop of the region, its value before the region neither known nor only XORed into:
  // its old value is an argument, its field is its final value, stored by exchange (Control.cpp).
  bool exchanged = false;
};

// The bits a variable may hold: up to its highest bit that may be 1.
unsigned supportWidth(const Storage &storage) {
  return std::max(1u, unsigned(64 - countl_zero(storage.support)));
}

class RegionWhole {
public:
  RegionWhole(Function &function, const std::vector<BasicBlock *> &blocks, BasicBlock *exit, ControlAnalysis &analysis)
      : fn(function), analysis(analysis), blocks(blocks), inRegion(blocks.begin(), blocks.end()), exit(exit) {}

  bool run();

private:
  Function &fn;
  ControlAnalysis &analysis;
  std::vector<BasicBlock *> blocks;
  std::set<const BasicBlock *> inRegion;
  BasicBlock *exit;
  std::vector<Written> written;
  std::vector<const Storage *> read; // qubit variables the region reads and does not write
  std::vector<const Written *> exchanged; // the written variables whose old values are arguments
  std::vector<Value *> inputs;       // values defined outside the region that it uses
  std::map<const BasicBlock *, std::set<const BasicBlock *>> later; // blocks reachable inside the region

  bool inside(const Value *value) const {
    const auto *inst = dyn_cast<Instruction>(value);
    return inst && inRegion.count(inst->getParent());
  }
  bool accumulated(const Storage &storage) const;
  bool cyclic() const;
  bool precedes(const Instruction *a, const Instruction *b) const;
  std::map<const Storage *, StoreInst *> movedStores() const;
  Value *finalValue(const Written &field, IRBuilder<> &b, ValueToValueMapTy &map) const;
  Function *copy(const std::vector<std::vector<const Written *>> &words, unsigned bits);
  std::vector<Value *> arguments(IRBuilder<> &b) const;
  void xorInto(IRBuilder<> &b, AllocaInst *target, Type *type, Value *value) const;
  void removeStores();
};

// Whether every value the region computes from `storage` is `y XOR d`, d not reading y, at the width of what
// y may hold: loads of all of y, XOR with a value that does not read y, extensions, truncations that keep those
// bits, masks that keep those bits, `!=` and `==` between values of at most one bit, phis and selects of such values on conditions
// that do not read y. Stores into y store such values, and nothing else uses them. Then the region's control
// flow does not depend on y, and its final value is y XOR its final value from 0.
bool RegionWhole::accumulated(const Storage &storage) const {
  const DataLayout &layout = fn.getParent()->getDataLayout();
  unsigned width = supportWidth(storage);
  // Everything in the region computed from a load of y.
  std::set<const Value *> reads;
  std::vector<const Value *> work;
  for (BasicBlock *block : blocks) {
    for (Instruction &inst : *block) {
      if (const auto *load = dyn_cast<LoadInst>(&inst)) {
        std::optional<Access> source = analysis.access(load->getPointerOperand());
        if (source && source->storage == &storage) {
          if (source->offset != 0 || load->getType()->getIntegerBitWidth() < width) {
            return false;
          }
          reads.insert(load);
          work.push_back(load);
        }
      }
    }
  }
  while (!work.empty()) {
    const Value *value = work.back();
    work.pop_back();
    for (const User *user : value->users()) {
      if (!inside(user)) {
        return false; // used after the region
      }
      if (!cast<Instruction>(user)->getType()->isVoidTy() && reads.insert(user).second) {
        work.push_back(user);
      }
    }
  }
  // How many low bits each such value may have set, to a fixed point: y's own width, widened by XOR.
  std::map<const Value *, unsigned> active;
  auto activeOf = [&](const Value *value) {
    return reads.count(value) ? active[value] : unsigned(computeKnownBits(value, layout).countMaxActiveBits());
  };
  for (bool changed = true; changed;) {
    changed = false;
    for (const Value *value : reads) {
      const auto *inst = dyn_cast<Instruction>(value);
      unsigned bits = width;
      if (isa<LoadInst>(inst)) {
        bits = width;
      } else if (inst->getOpcode() == Instruction::Xor || isa<PHINode>(inst) || isa<SelectInst>(inst)) {
        bits = 0;
        for (unsigned i = isa<SelectInst>(inst) ? 1 : 0; i < inst->getNumOperands(); ++i) {
          bits = std::max(bits, activeOf(inst->getOperand(i)));
        }
      } else if (inst->getOpcode() == Instruction::And) {
        bits = std::min(activeOf(inst->getOperand(0)), activeOf(inst->getOperand(1)));
      } else if (isa<CastInst>(inst) || isa<FreezeInst>(inst)) {
        bits = std::min(activeOf(inst->getOperand(0)), inst->getType()->getIntegerBitWidth());
      } else if (isa<ICmpInst>(inst)) {
        bits = 1;
      }
      if (bits > active[value]) {
        active[value] = bits;
        changed = true;
      }
    }
  }
  for (const Value *value : reads) {
    for (const User *user : value->users()) {
      const auto *inst = cast<Instruction>(user);
      bool ok = false;
      if (const auto *store = dyn_cast<StoreInst>(inst)) {
        std::optional<Access> target = analysis.access(store->getPointerOperand());
        ok = target && target->storage == &storage && store->getValueOperand() == value;
      } else if (inst->getOpcode() == Instruction::Xor) {
        ok = !reads.count(inst->getOperand(0)) || !reads.count(inst->getOperand(1));
      } else if (isa<ZExtInst>(inst) || isa<FreezeInst>(inst)) {
        ok = true;
      } else if (isa<TruncInst>(inst)) {
        ok = inst->getType()->getIntegerBitWidth() >= width;
      } else if (const auto *compare = dyn_cast<ICmpInst>(inst)) {
        // On values of at most one bit, `!=` is XOR and `==` is XOR with 1 (how LLVM writes `y ^= b` on a _Bool).
        const Value *other = compare->getOperand(compare->getOperand(0) == value ? 1 : 0);
        ok = width == 1 && compare->isEquality() && !reads.count(other) && activeOf(value) <= 1 &&
             activeOf(other) <= 1;
      } else if (inst->getOpcode() == Instruction::And) {
        // A mask that keeps every bit y may hold: (y ^ d) & m = y ^ (d & m).
        const auto *mask = dyn_cast<ConstantInt>(inst->getOperand(1));
        ok = mask && inst->getOperand(0) == value && mask->getValue().countr_one() >= width;
      } else if (const auto *phi = dyn_cast<PHINode>(inst)) {
        ok = std::all_of(phi->incoming_values().begin(), phi->incoming_values().end(),
                         [&](const Value *in) { return reads.count(in) != 0; });
      } else if (const auto *select = dyn_cast<SelectInst>(inst)) {
        ok = !reads.count(select->getCondition()) && reads.count(select->getTrueValue()) &&
             reads.count(select->getFalseValue());
      }
      if (!ok) {
        return false;
      }
    }
  }
  // Every store into y stores such a value: anything else would overwrite y.
  for (Instruction *mutation : storage.mutations) {
    if (inside(mutation) && !reads.count(cast<StoreInst>(mutation)->getValueOperand())) {
      return false;
    }
  }
  return true;
}

bool RegionWhole::cyclic() const {
  for (BasicBlock *block : blocks) {
    if (later.at(block).count(block)) {
      return true;
    }
  }
  return false;
}

bool RegionWhole::precedes(const Instruction *a, const Instruction *b) const {
  if (a->getParent() == b->getParent()) {
    return a->comesBefore(b) || later.at(a->getParent()).count(a->getParent()) != 0;
  }
  return later.at(a->getParent()).count(b->getParent()) != 0;
}

// The values the copy receives, in its parameters' order: the region's inputs, then the registers of the
// variables it only reads and the old values of those it exchanges, loaded here.
std::vector<Value *> RegionWhole::arguments(IRBuilder<> &b) const {
  std::vector<Value *> args(inputs.begin(), inputs.end());
  for (const Storage *storage : read) {
    args.push_back(b.CreateLoad(b.getIntNTy(storage->bits), storage->alloca));
  }
  for (const Written *field : exchanged) {
    args.push_back(b.CreateLoad(b.getIntNTy(field->storage->bits), field->storage->alloca));
  }
  return args;
}

// Stores where the region ends of values the region computes: LLVM moves a loop's stores out of the loop
// (scalar promotion), leaving the value in a register and one store after the loop. Such a store is a write of
// the region when the exit block is entered from the region only and begins with it (after phis and code that
// touches no memory), it stores all of a qubit variable the region does not touch, whose value before the region
// is known, and its value is computed in the region or is a phi of the exit block.
std::map<const Storage *, StoreInst *> RegionWhole::movedStores() const {
  std::map<const Storage *, StoreInst *> moved;
  if (!std::all_of(pred_begin(exit), pred_end(exit), [&](const BasicBlock *from) { return inRegion.count(from); })) {
    return moved;
  }
  const DataLayout &layout = fn.getParent()->getDataLayout();
  Instruction *first = &*blocks.front()->getFirstNonPHIIt();
  for (Instruction &inst : *exit) {
    auto *store = dyn_cast<StoreInst>(&inst);
    if (!store) {
      if (isa<PHINode>(inst) || isa<DbgInfoIntrinsic>(inst) || !inst.mayReadOrWriteMemory()) {
        continue;
      }
      break;
    }
    std::optional<Access> place = analysis.access(store->getPointerOperand());
    if (!store->isSimple() || !place || place->offset != 0 ||
        layout.getTypeStoreSizeInBits(store->getValueOperand()->getType()) != place->storage->bits ||
        moved.count(place->storage)) {
      break;
    }
    const Value *value = store->getValueOperand();
    bool computed = isa<PHINode>(value) ? cast<PHINode>(value)->getParent() == exit : inside(value);
    bool untouched = std::none_of(blocks.begin(), blocks.end(), [&](const BasicBlock *block) {
      return std::any_of(block->begin(), block->end(), [&](const Instruction &use) {
        const Value *pointer = isa<LoadInst>(use)    ? cast<LoadInst>(use).getPointerOperand()
                               : isa<StoreInst>(use) ? cast<StoreInst>(use).getPointerOperand()
                                                     : nullptr;
        return pointer && analysis.basedOn(pointer) == place->storage;
      });
    });
    Fact fact = analysis.before(first, place->storage);
    bool known = fact.kind == Fact::Known && !inside(fact.value) &&
                 (isa<Constant>(fact.value) || analysis.dominators.dominates(fact.value, first));
    if (!computed || !untouched || !known) {
      break;
    }
    moved[place->storage] = store;
  }
  return moved;
}

// A field's value where the region ends, in the copy's exit block (`b`), whose predecessors are the copies of the
// region's blocks that leave it.
Value *RegionWhole::finalValue(const Written &field, IRBuilder<> &b, ValueToValueMapTy &map) const {
  Type *type = IntegerType::get(fn.getContext(), field.storage->bits);
  if (!field.moved) {
    return b.CreateLoad(type, map[field.storage->alloca]);
  }
  if (field.incoming.empty()) {
    return map[field.moved->getValueOperand()];
  }
  IRBuilder<> top(b.GetInsertBlock(), b.GetInsertBlock()->begin());
  PHINode *copy = top.CreatePHI(type, unsigned(field.incoming.size()));
  for (const auto &[from, in] : field.incoming) {
    Value *mapped = map.lookup(in);
    copy->addIncoming(mapped ? mapped : in, cast<BasicBlock>(map[from]));
  }
  return copy;
}

// A copy of the region as an ordinary function: each qubit variable it touches is a private local, starting at
// the variable's value (read only), its known value or 0 (written); it returns `final XOR start` of each field,
// in one word of `bits` bits. With several words of 64 bits, a last parameter names the word to return.
Function *RegionWhole::copy(const std::vector<std::vector<const Written *>> &words, unsigned bits) {
  LLVMContext &context = fn.getContext();
  std::vector<Type *> types;
  for (Value *input : inputs) {
    types.push_back(input->getType());
  }
  for (const Storage *storage : read) {
    types.push_back(IntegerType::get(context, storage->bits));
  }
  for (const Written *field : exchanged) {
    types.push_back(IntegerType::get(context, field->storage->bits));
  }
  if (words.size() > 1) {
    types.push_back(Type::getInt32Ty(context));
  }
  Type *result = IntegerType::get(context, bits);
  Function *copy = Function::Create(FunctionType::get(result, types, false), GlobalValue::InternalLinkage,
                                    fn.getName() + ".whole", fn.getParent());
  BasicBlock *top = BasicBlock::Create(context, "entry", copy);
  IRBuilder<> b(top);
  ValueToValueMapTy map;
  for (size_t i = 0; i < inputs.size(); ++i) {
    map[inputs[i]] = copy->getArg(unsigned(i));
  }
  // The private locals and their starting values. A qubit variable the region does not touch is not used by
  // the copy; one it touches is its private local, so that positions that are not constant follow it too.
  std::map<const Storage *, AllocaInst *> locals;
  std::map<const Storage *, Value *> starts;
  for (size_t i = 0; i < read.size(); ++i) {
    Type *type = IntegerType::get(context, read[i]->bits);
    locals[read[i]] = b.CreateAlloca(type);
    starts[read[i]] = copy->getArg(unsigned(inputs.size() + i));
  }
  for (const Written &w : written) {
    Type *type = IntegerType::get(context, w.storage->bits);
    locals[w.storage] = b.CreateAlloca(type);
    Value *start = ConstantInt::get(type, 0);
    if (w.exchanged) {
      auto at = std::find(exchanged.begin(), exchanged.end(), &w) - exchanged.begin();
      start = copy->getArg(unsigned(inputs.size() + read.size() + at));
    } else if (w.known) {
      Value *known = isa<Constant>(w.known) ? w.known : static_cast<Value *>(map[w.known]);
      start = b.CreateZExtOrTrunc(known, type);
    }
    starts[w.storage] = start;
  }
  for (const auto &[storage, local] : locals) {
    b.CreateStore(starts[storage], local);
  }
  for (const auto &storage : analysis.storages) {
    auto local = locals.find(storage.get());
    map[storage->alloca] = local != locals.end() ? static_cast<Value *>(local->second)
                                                 : PoisonValue::get(storage->alloca->getType());
  }
  std::vector<BasicBlock *> cloned;
  for (BasicBlock *block : blocks) {
    BasicBlock *clone = CloneBasicBlock(block, map, "", copy);
    map[block] = clone;
    cloned.push_back(clone);
  }
  remapInstructionsInBlocks(cloned, map);
  auto *clonedEntry = cast<BasicBlock>(map[blocks.front()]);
  BranchInst *start = b.CreateBr(clonedEntry);
  for (PHINode &phi : clonedEntry->phis()) {
    for (unsigned i = 0; i < phi.getNumIncomingValues(); ++i) {
      if (phi.getIncomingBlock(i)->getParent() != copy) {
        phi.setIncomingBlock(i, top);
      }
    }
  }
  // Loads and stores of qubit variables use the private locals.
  b.SetInsertPoint(start);
  for (BasicBlock *block : blocks) {
    for (Instruction &inst : *block) {
      Value *pointer = isa<LoadInst>(inst)    ? cast<LoadInst>(inst).getPointerOperand()
                       : isa<StoreInst>(inst) ? cast<StoreInst>(inst).getPointerOperand()
                                              : nullptr;
      std::optional<Access> source = pointer ? analysis.access(pointer) : std::nullopt;
      if (!source) {
        continue;
      }
      Value *local = locals.at(source->storage);
      if (source->offset != 0) {
        local = b.CreateConstInBoundsGEP1_64(b.getInt8Ty(), local, source->offset);
      }
      auto *clone = cast<Instruction>(map[&inst]);
      clone->setOperand(isa<LoadInst>(inst) ? 0 : 1, local);
    }
  }
  // Edges out of the region return the fields.
  BasicBlock *out = BasicBlock::Create(context, "exit", copy);
  for (BasicBlock *clone : cloned) {
    Instruction *term = clone->getTerminator();
    for (unsigned i = 0; i < term->getNumSuccessors(); ++i) {
      if (term->getSuccessor(i)->getParent() != copy) {
        term->setSuccessor(i, out);
      }
    }
  }
  b.SetInsertPoint(out);
  // Every word is computed, then the one asked for chosen, so that each word takes the same steps.
  std::vector<Value *> values;
  for (const std::vector<const Written *> &fields : words) {
    Value *word = ConstantInt::get(result, 0);
    for (const Written *field : fields) {
      // An exchanged variable's field is its final value; any other's, final XOR start.
      Value *change = finalValue(*field, b, map);
      if (!field->exchanged) {
        change = b.CreateXor(change, starts[field->storage]);
      }
      word = b.CreateOr(word, b.CreateShl(b.CreateZExt(change, result), field->offset));
    }
    values.push_back(word);
  }
  Value *chosen = values.back();
  for (size_t i = values.size() - 1; i-- > 0;) {
    chosen = b.CreateSelect(b.CreateICmpEQ(copy->getArg(copy->arg_size() - 1), b.getInt32(unsigned(i))), values[i],
                            chosen);
  }
  b.CreateRet(chosen);
  // What named a qubit variable of the control program is unused now; the copy has no debug information.
  for (BasicBlock *clone : cloned) {
    for (Instruction &inst : make_early_inc_range(*clone)) {
      if (isa<DbgInfoIntrinsic>(inst)) {
        inst.eraseFromParent();
      }
    }
  }
  for (bool changed = true; changed;) {
    changed = false;
    for (BasicBlock *clone : cloned) {
      for (Instruction &inst : make_early_inc_range(reverse(*clone))) {
        if (isInstructionTriviallyDead(&inst)) {
          inst.eraseFromParent();
          changed = true;
        }
      }
    }
  }
  removeUnreachableBlocks(*copy);
  for (Instruction &inst : instructions(*copy)) {
    inst.setDebugLoc(DebugLoc()); // the copy has no debug information of its own, and its loop metadata would
    inst.setMetadata(LLVMContext::MD_loop, nullptr); // name the control function's
  }
  std::vector<AllocaInst *> promote;
  for (const auto &[storage, local] : locals) {
    if (isAllocaPromotable(local)) {
      promote.push_back(local); // the rest (read in part) is left to the optimiser
    }
  }
  DominatorTree dominators(*copy);
  PromoteMemToReg(promote, dominators);
  std::string problems;
  raw_string_ostream stream(problems);
  if (verifyFunction(*copy, &stream)) {
    fail("internal: a copy of a region applied whole is broken:\n" + stream.str());
  }
  simplifyDeep(*copy);
  copy->setOnlyReadsMemory(); // the region writes no memory: outlineAround refused it
  copy->setDoesNotThrow();
  return copy;
}

// target ^= value, as a store the control program lowers to `apply` (Control.cpp).
void RegionWhole::xorInto(IRBuilder<> &b, AllocaInst *target, Type *type, Value *value) const {
  Value *old = b.CreateLoad(type, target);
  b.CreateStore(b.CreateXor(old, value), target);
}

// The region's stores into qubit variables leave it. What remains computes only the values the region passes
// on; if there are none, it goes.
void RegionWhole::removeStores() {
  std::set<const Instruction *> readAfterChange;
  std::vector<StoreInst *> stores;
  for (BasicBlock *block : blocks) {
    for (Instruction &inst : *block) {
      if (auto *store = dyn_cast<StoreInst>(&inst); store && analysis.basedOn(store->getPointerOperand())) {
        stores.push_back(store);
      }
    }
  }
  for (BasicBlock *block : blocks) {
    for (Instruction &inst : *block) {
      auto *load = dyn_cast<LoadInst>(&inst);
      const Storage *source = load ? analysis.basedOn(load->getPointerOperand()) : nullptr;
      for (StoreInst *store : stores) {
        if (source && analysis.basedOn(store->getPointerOperand()) == source && precedes(store, load)) {
          readAfterChange.insert(load);
        }
      }
    }
  }
  for (StoreInst *store : stores) {
    store->eraseFromParent();
  }
  // What is left is live if it has an effect, a successor to choose, or a use after the region, or feeds
  // what is live; the rest goes, cycles of phis included.
  std::set<Instruction *> live;
  std::vector<Instruction *> work;
  for (BasicBlock *block : blocks) {
    for (Instruction &inst : *block) {
      bool usedAfter = std::any_of(inst.user_begin(), inst.user_end(), [&](const User *user) { return !inside(user); });
      if (inst.isTerminator() || inst.mayHaveSideEffects() || usedAfter) {
        live.insert(&inst);
        work.push_back(&inst);
      }
    }
  }
  while (!work.empty()) {
    Instruction *inst = work.back();
    work.pop_back();
    for (Use &use : inst->operands()) {
      if (auto *operand = dyn_cast<Instruction>(use.get()); operand && inside(operand) && live.insert(operand).second) {
        work.push_back(operand);
      }
    }
  }
  std::vector<Instruction *> dead;
  for (BasicBlock *block : blocks) {
    for (Instruction &inst : *block) {
      if (!live.count(&inst)) {
        dead.push_back(&inst);
      }
    }
  }
  for (Instruction *inst : dead) {
    inst->dropAllReferences();
  }
  for (Instruction *inst : dead) {
    readAfterChange.erase(inst);
    inst->eraseFromParent();
  }
  // The region passes on a value when one of its instructions is used after it, and when a phi where it ends
  // selects by the edge it leaves on: that phi's value is decided by the path through the region even where
  // every incoming value is a constant or defined before the region. The exit (`after`) is entered from the
  // region's blocks only, so each of its phis that does not take one value on every edge is such a phi (the
  // unused ones are gone, run()).
  bool liveOut = std::any_of(exit->phis().begin(), exit->phis().end(),
                             [](PHINode &phi) { return phi.hasConstantValue() == nullptr; });
  for (BasicBlock *block : blocks) {
    for (Instruction &inst : *block) {
      for (User *user : inst.users()) {
        liveOut = liveOut || !inRegion.count(cast<Instruction>(user)->getParent());
      }
    }
  }
  if (!liveOut) {
    BasicBlock *entry = blocks.front();
    for (BasicBlock *pred : SmallVector<BasicBlock *, 4>(predecessors(entry))) {
      if (!inRegion.count(pred)) {
        pred->getTerminator()->replaceSuccessorWith(entry, exit);
      }
    }
    DeleteDeadBlocks(blocks);
    return;
  }
  for (BasicBlock *block : blocks) {
    for (Instruction &inst : *block) {
      if (readAfterChange.count(&inst)) {
        notHandled("the value of qubit variable " + analysis.basedOn(cast<LoadInst>(inst).getPointerOperand())->name +
                       " read here, after the code under a condition that depends on a qubit wrote it, is used "
                       "after that code ends",
                   &inst);
      }
    }
  }
}

bool RegionWhole::run() {
  // Which blocks each block reaches inside the region.
  for (BasicBlock *block : blocks) {
    std::vector<BasicBlock *> work{block};
    std::set<const BasicBlock *> &reach = later[block];
    while (!work.empty()) {
      BasicBlock *from = work.back();
      work.pop_back();
      for (BasicBlock *succ : successors(from)) {
        if (inRegion.count(succ) && reach.insert(succ).second) {
          work.push_back(succ);
        }
      }
    }
  }
  // The variables the region writes and reads.
  std::set<const Storage *> writes, reads;
  for (BasicBlock *block : blocks) {
    for (Instruction &inst : *block) {
      if (auto *store = dyn_cast<StoreInst>(&inst)) {
        if (const Storage *target = analysis.basedOn(store->getPointerOperand())) {
          writes.insert(target);
        }
      } else if (auto *load = dyn_cast<LoadInst>(&inst)) {
        if (const Storage *source = analysis.basedOn(load->getPointerOperand())) {
          reads.insert(source);
        }
      }
    }
  }
  Instruction *first = &*blocks.front()->getFirstNonPHIIt();
  std::map<const Storage *, StoreInst *> moved = movedStores();
  for (const auto &storage : analysis.storages) {
    if (auto found = moved.find(storage.get()); found != moved.end()) {
      Written w{storage.get(), analysis.before(first, storage.get()).value};
      w.moved = found->second;
      if (auto *phi = dyn_cast<PHINode>(w.moved->getValueOperand())) {
        for (unsigned i = 0; i < phi->getNumIncomingValues(); ++i) {
          w.incoming.emplace_back(phi->getIncomingBlock(i), phi->getIncomingValue(i));
        }
      }
      written.push_back(w);
      continue;
    }
    if (!writes.count(storage.get())) {
      if (reads.count(storage.get())) {
        read.push_back(storage.get());
      }
      continue;
    }
    Written w{storage.get()};
    Fact fact = analysis.before(first, storage.get());
    if (fact.kind == Fact::Known && !inside(fact.value) &&
        (isa<Constant>(fact.value) || analysis.dominators.dominates(fact.value, first))) {
      w.known = fact.value;
    } else if (!accumulated(*storage)) {
      if (!cyclic()) {
        return false;
      }
      // Read and overwritten in a loop: a store by exchange, with the old value among the arguments.
      w.exchanged = true;
    }
    written.push_back(w);
  }
  for (const Written &w : written) {
    if (w.exchanged) {
      exchanged.push_back(&w);
    }
  }

  // One edge into the region's entry, so that its phis have one value from outside; one block after it.
  BasicBlock *entry = blocks.front();
  SetVector<BasicBlock *> outside, insideExit;
  for (BasicBlock *pred : predecessors(entry)) {
    if (!inRegion.count(pred)) {
      outside.insert(pred);
    }
  }
  if (outside.size() > 1) {
    SplitBlockPredecessors(entry, outside.getArrayRef(), ".whole");
  }
  for (BasicBlock *pred : predecessors(exit)) {
    if (inRegion.count(pred)) {
      insideExit.insert(pred);
    }
  }
  BasicBlock *after = SplitBlockPredecessors(exit, insideExit.getArrayRef(), ".after");
  if (!after) {
    fail("cannot make a block after a region applied whole");
  }
  exit = after;

  // The values the region uses from outside: inputs of the copy (pointers into qubit variables excepted).
  std::set<const Value *> seen;
  for (const Written &w : written) {
    if (w.known && !isa<Constant>(w.known) && seen.insert(w.known).second) {
      inputs.push_back(w.known);
    }
    // A moved store's phi may take a value from outside the region on an edge out of it.
    for (const auto &[from, in] : w.incoming) {
      if ((isa<Instruction>(in) || isa<Argument>(in)) && !inside(in) && seen.insert(in).second) {
        inputs.push_back(in);
      }
    }
  }
  for (BasicBlock *block : blocks) {
    for (Instruction &inst : *block) {
      for (Use &use : inst.operands()) {
        Value *value = use.get();
        if ((isa<Instruction>(value) || isa<Argument>(value)) && !inside(value) && !analysis.access(value) &&
            seen.insert(value).second) {
          inputs.push_back(value);
        }
      }
    }
  }

  // Fields, grouped into result words of at most 64 bits: one register each, all of them returned by one
  // applied function, which takes the word's number when there are several.
  // An exchanged variable's field is a word of its own: its fresh register holds its old value afterwards.
  std::vector<std::vector<const Written *>> words;
  unsigned used = maxResultBits;
  for (Written &w : written) {
    if (w.exchanged) {
      continue;
    }
    if (used + w.storage->bits > maxResultBits) {
      words.emplace_back();
      used = 0;
    }
    w.offset = used;
    used += w.storage->bits;
    words.back().push_back(&w);
  }
  for (const Written *field : exchanged) {
    words.push_back({field});
  }
  // A register of whole bytes, as a qubit variable is (Taint.h): 8, 16, 32 or 64 bits.
  auto width = [](const std::vector<const Written *> &fields) {
    return std::max(8u, unsigned(PowerOf2Ceil(fields.back()->offset + fields.back()->storage->bits)));
  };
  Function *whole = copy(words, words.size() > 1 ? maxResultBits : width(words.front()));

  LLVMContext &context = fn.getContext();
  Module &module = *fn.getParent();
  const Instruction *site = writes.empty() ? blocks.front()->getTerminator() : (*writes.begin())->mutations.front();
  if (words.size() > 1 || words.front().size() > 1) {
    requireCallerIndependent(*whole, *site);
  }
  IRBuilder<> b(after->getTerminator());
  b.SetCurrentDebugLocation(site->getDebugLoc());
  const std::vector<Value *> loaded = arguments(b);
  auto evaluateWord = [&](size_t index) {
    std::vector<Value *> args = loaded;
    if (words.size() > 1) {
      args.push_back(b.getInt32(unsigned(index)));
    }
    return b.CreateCall(whole, args);
  };
  for (size_t index = 0; index < words.size(); ++index) {
    const std::vector<const Written *> &fields = words[index];
    if (fields.front()->exchanged) {
      continue; // below, after every result that reads the old values
    }
    auto evaluate = [&] { return evaluateWord(index); };
    if (fields.size() == 1) {
      const Storage &target = *fields.front()->storage;
      Type *type = IntegerType::get(context, target.bits);
      xorInto(b, target.alloca, type, b.CreateZExtOrTrunc(evaluate(), type));
      continue;
    }
    // Several results: into a fresh register, a CNOT from each of its bits into its variable, and the same
    // computation again to return the register to 0.
    Type *type = IntegerType::get(context, width(fields));
    IRBuilder<> top(&*fn.getEntryBlock().getFirstInsertionPt());
    AllocaInst *results = top.CreateAlloca(type, nullptr, fn.getName() + ".results");
    results->setMetadata(qubitMetadata, MDNode::get(context, {}));
    Value *first = evaluate();
    xorInto(b, results, type, b.CreateZExtOrTrunc(first, type));
    FunctionCallee bit = module.getOrInsertFunction("bennett_bit", PointerType::get(context, 0),
                                                    PointerType::get(context, 0), b.getInt32Ty());
    FunctionCallee x = module.getOrInsertFunction("bennett_x", b.getVoidTy(), PointerType::get(context, 0));
    for (const Written *field : fields) {
      for (unsigned i = 0; i < field->storage->bits; ++i) {
        if (!(field->storage->support >> i & 1)) {
          continue; // a bit the variable never holds, so never part of the change
        }
        Value *control = b.CreateCall(bit, {results, b.getInt32(field->offset + i)});
        Value *target = b.CreateCall(bit, {field->storage->alloca, b.getInt32(i)});
        b.CreateCall(x, {target}, {OperandBundleDef(qubitControlBundle, std::vector<Value *>{control})});
      }
    }
    xorInto(b, results, type, b.CreateZExtOrTrunc(evaluate(), type));
    // A variable with a known start now holds start XOR its field, a value computed from registers unchanged here:
    // recorded, so that a later overwrite can clear it by computing it again (a result XORed in directly is a store
    // of that value already).
    for (const Written *field : fields) {
      if (!field->known) {
        continue;
      }
      Type *bits = IntegerType::get(context, field->storage->bits);
      Value *change = b.CreateTrunc(b.CreateLShr(first, field->offset), bits);
      recordHolds(after->getTerminator(), field->storage->alloca,
                  b.CreateXor(b.CreateZExtOrTrunc(field->known, bits), change));
    }
  }
  // The exchanged variables: each final value is stored whole into its variable, the stores one group, which the
  // control program carries out as stores by exchange: every final value applied into a fresh register first,
  // then each exchanged with its variable, then each register cleared (Control.cpp). Every value is computed
  // before the first store, from the old values loaded above.
  if (!exchanged.empty()) {
    MDNode *group = MDNode::getDistinct(context, {});
    std::vector<std::pair<const Written *, Value *>> finals;
    for (size_t index = 0; index < words.size(); ++index) {
      if (words[index].front()->exchanged) {
        const Written *field = words[index].front();
        Type *type = IntegerType::get(context, field->storage->bits);
        finals.emplace_back(field, b.CreateZExtOrTrunc(evaluateWord(index), type));
      }
    }
    for (const auto &[field, value] : finals) {
      const std::vector<Instruction *> &mutations = field->storage->mutations;
      auto site = std::find_if(mutations.begin(), mutations.end(), [&](const Instruction *m) { return inside(m); });
      b.SetCurrentDebugLocation((site != mutations.end() ? *site : mutations.front())->getDebugLoc());
      b.CreateStore(value, field->storage->alloca)->setMetadata(overwriteMetadata, group);
    }
  }
  // The moved stores are done by the results; what they stored is no longer used after the region.
  for (Written &w : written) {
    if (w.moved) {
      w.moved->eraseFromParent();
      w.moved = nullptr;
    }
  }
  for (BasicBlock *block : {after->getSingleSuccessor(), after}) { // the exit's phis use those of `after`
    for (bool changed = true; block && changed;) {
      changed = false;
      for (PHINode &phi : make_early_inc_range(block->phis())) {
        if (phi.use_empty()) {
          phi.eraseFromParent();
          changed = true;
        }
      }
    }
  }
  removeStores();
  return true;
}

} // namespace

bool hasGates(const std::vector<BasicBlock *> &blocks) {
  for (const BasicBlock *block : blocks) {
    for (const Instruction &inst : *block) {
      if (isGateCall(&inst)) {
        return true;
      }
    }
  }
  return false;
}

bool applyRegionWhole(Function &function, const std::vector<BasicBlock *> &blocks, BasicBlock *exit,
                      ControlAnalysis &analysis) {
  return RegionWhole(function, blocks, exit, analysis).run();
}

} // namespace bennett
