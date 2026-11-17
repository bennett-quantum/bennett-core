#include "RegionResults.h"

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
  std::vector<Value *> inputs;       // values defined outside the region that it uses
  std::map<const BasicBlock *, std::set<const BasicBlock *>> later; // blocks reachable inside the region

  bool inside(const Value *value) const {
    const auto *inst = dyn_cast<Instruction>(value);
    return inst && inRegion.count(inst->getParent());
  }
  bool accumulated(const Storage &storage) const;
  bool cyclic() const;
  bool precedes(const Instruction *a, const Instruction *b) const;
  Function *copy(const std::vector<const Written *> &fields, unsigned bits);
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
// variables it only reads, loaded here.
std::vector<Value *> RegionWhole::arguments(IRBuilder<> &b) const {
  std::vector<Value *> args(inputs.begin(), inputs.end());
  for (const Storage *storage : read) {
    args.push_back(b.CreateLoad(b.getIntNTy(storage->bits), storage->alloca));
  }
  return args;
}

// A copy of the region as an ordinary function: each qubit variable it touches is a private local, starting at
// the variable's value (read only), its known value or 0 (written); it returns `final XOR start` of each field.
Function *RegionWhole::copy(const std::vector<const Written *> &fields, unsigned bits) {
  LLVMContext &context = fn.getContext();
  std::vector<Type *> types;
  for (Value *input : inputs) {
    types.push_back(input->getType());
  }
  for (const Storage *storage : read) {
    types.push_back(IntegerType::get(context, storage->bits));
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
    if (w.known) {
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
  Value *word = ConstantInt::get(result, 0);
  for (const Written *field : fields) {
    Type *type = IntegerType::get(context, field->storage->bits);
    Value *change = b.CreateXor(b.CreateLoad(type, locals[field->storage]), starts[field->storage]);
    word = b.CreateOr(word, b.CreateShl(b.CreateZExt(change, result), field->offset));
  }
  b.CreateRet(word);
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
  bool liveOut = false;
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
  for (const auto &storage : analysis.storages) {
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
      if (cyclic()) {
        notHandled("an update of qubit variable " + storage->name +
                       " in a loop under the control of a value that depends on a qubit, whose value before the "
                       "loop is not known and which the loop does more with than XOR into it (its old value "
                       "would have to be kept: not built yet)",
                   storage->mutations.front());
      }
      return false;
    }
    written.push_back(w);
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

  // Fields, grouped into result words of at most 64 bits.
  std::vector<std::vector<const Written *>> words(1);
  unsigned used = 0;
  for (Written &w : written) {
    if (used + w.storage->bits > maxResultBits) {
      words.emplace_back();
      used = 0;
    }
    w.offset = used;
    used += w.storage->bits;
    words.back().push_back(&w);
  }

  LLVMContext &context = fn.getContext();
  Module &module = *fn.getParent();
  IRBuilder<> b(after->getTerminator());
  b.SetCurrentDebugLocation(writes.empty() ? DebugLoc() : (*writes.begin())->mutations.front()->getDebugLoc());
  for (const std::vector<const Written *> &fields : words) {
    // A register of whole bytes, as a qubit variable is (Taint.h): 8, 16, 32 or 64 bits.
    unsigned bits = std::max(8u, unsigned(PowerOf2Ceil(fields.back()->offset + fields.back()->storage->bits)));
    Function *whole = copy(fields, bits);
    if (fields.size() == 1) {
      const Storage &target = *fields.front()->storage;
      Type *type = IntegerType::get(context, target.bits);
      xorInto(b, target.alloca, type, b.CreateZExtOrTrunc(b.CreateCall(whole, arguments(b)), type));
      continue;
    }
    // Several results: into a fresh register, a CNOT from each of its bits into its variable, and the same
    // computation again to return the register to 0.
    Type *type = IntegerType::get(context, bits);
    IRBuilder<> top(&*fn.getEntryBlock().getFirstInsertionPt());
    AllocaInst *results = top.CreateAlloca(type, nullptr, fn.getName() + ".results");
    results->setMetadata(qubitMetadata, MDNode::get(context, {}));
    xorInto(b, results, type, b.CreateCall(whole, arguments(b)));
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
    xorInto(b, results, type, b.CreateCall(whole, arguments(b)));
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
