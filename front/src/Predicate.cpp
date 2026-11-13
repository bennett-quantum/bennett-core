#include "Predicate.h"

#include "Diagnostic.h"
#include "Markers.h"
#include "Normalise.h"
#include "Taint.h"

#include <llvm/ADT/PostOrderIterator.h>
#include <llvm/ADT/SetVector.h>
#include <llvm/Analysis/ValueTracking.h>
#include <llvm/IR/Constants.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/Module.h>
#include <llvm/IR/Verifier.h>
#include <llvm/Support/KnownBits.h>
#include <llvm/Support/raw_ostream.h>
#include <llvm/Transforms/Utils/BasicBlockUtils.h>
#include <llvm/Transforms/Utils/Cloning.h>
#include <llvm/Transforms/Utils/Local.h>
#include <llvm/Transforms/Utils/ValueMapper.h>

#include <map>
#include <set>

using namespace llvm;

namespace bennett {

namespace {

bool isQubitStore(const Instruction &inst, const ControlAnalysis &analysis) {
  const auto *store = dyn_cast<StoreInst>(&inst);
  return store && analysis.access(store->getPointerOperand());
}

bool isBitCall(const Value *value) {
  std::optional<Marker> marker = markerCall(value);
  return marker && marker->kind == MarkerKind::Bit;
}

// What a copy of the region returns where the operation is reached, and at the region's exit.
enum class Result {
  Reached,  // i1: the operation's condition (its own, if it already has one) / 0
  Change,   // the store's new value XOR the variable's value there / 0
  NewValue, // the store's new value / the variable's value
};

// A parameter of a copy of the region.
struct Param {
  enum Kind { Input, Current, Stale };
  Kind kind;
  Value *input = nullptr;           // Input: a value defined outside the region
  const Storage *storage = nullptr; // Current: a qubit variable, read where the operation is
  unsigned offset = 0;
  Type *type = nullptr;
  LoadInst *load = nullptr; // Stale: a load whose value another operation changes before this one
};

class Predicator {
public:
  Predicator(Function &function, const std::vector<BasicBlock *> &region, BasicBlock *exit,
             ControlAnalysis &analysis);
  void run();

private:
  Function &fn;
  ControlAnalysis &analysis;
  BasicBlock *entry;                // the region's entry
  std::vector<BasicBlock *> blocks; // in a topological order of the region's strongly connected components
  std::set<const BasicBlock *> inRegion;
  BasicBlock *exit;
  BasicBlock *after = nullptr; // between the region and its exit; the operations go here
  std::map<const BasicBlock *, std::set<const BasicBlock *>> later; // blocks reachable inside the region
  std::vector<Instruction *> ops;                                   // in the order they are moved
  std::vector<LoadInst *> loads;                                    // the region's loads of qubit variables
  std::map<const Instruction *, const Value *> knownValues;         // a store's target value, when known
  std::set<const Instruction *> readAfterChange; // loads that may read what an operation wrote

  // Orders the blocks and operations; refuses an operation on a cycle.
  void order();
  bool precedes(const Instruction *a, const Instruction *b) const;
  bool mutates(const Instruction *op, const Storage *storage) const;
  bool stale(const LoadInst *load, const Instruction *op) const;
  const Storage *targetOf(const Instruction *op) const;
  Function *copy(Instruction *op, Result result, const Value *known, std::vector<Param> &params);
  void refuseStale(Function *copy, const std::vector<Param> &params, const Instruction *op) const;
  bool readsVariable(Function *copy, const std::vector<Param> &params, const Storage *storage) const;
  CallInst *call(Function *copy, const std::vector<Param> &params, IRBuilder<> &b) const;
  Value *qubitPointer(Value *pointer, IRBuilder<> &b) const;
  void moveGate(CallBase *gate, IRBuilder<> &b);
  void moveStore(StoreInst *store, IRBuilder<> &b);
  void removeOperations();
};

Predicator::Predicator(Function &function, const std::vector<BasicBlock *> &region, BasicBlock *exit,
                       ControlAnalysis &analysis)
    : fn(function), analysis(analysis), entry(region.front()), inRegion(region.begin(), region.end()), exit(exit) {
  ReversePostOrderTraversal<Function *> order(&function);
  for (BasicBlock *block : order) {
    if (inRegion.count(block)) {
      blocks.push_back(block);
    }
  }
}

void Predicator::order() {
  // Which blocks each block reaches inside the region, by following edges to a fixed point.
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
  // An operation on a cycle may act more than once per run of the region: that needs a budgeted
  // construction this version does not have. Loops elsewhere in the region stay in the copies.
  for (Instruction *op : ops) {
    if (later[op->getParent()].count(op->getParent())) {
      notHandled("a loop under the control of a value that depends on a qubit, with quantum operations "
                 "inside (only a loop that LLVM unrolls fully is handled)",
                 op);
    }
  }
  // A topological order of the region's strongly connected components: a block that reaches another
  // without being reached by it has fewer such predecessors. Ties keep the reverse post-order.
  std::map<const BasicBlock *, size_t> before;
  for (BasicBlock *block : blocks) {
    for (BasicBlock *other : blocks) {
      before[block] += later[other].count(block) && !later[block].count(other) ? 1 : 0;
    }
  }
  std::stable_sort(blocks.begin(), blocks.end(),
                   [&](const BasicBlock *a, const BasicBlock *b) { return before[a] < before[b]; });
  ops.clear();
  for (BasicBlock *block : blocks) {
    for (Instruction &inst : *block) {
      if (isGateCall(&inst) || isQubitStore(inst, analysis)) {
        ops.push_back(&inst);
      }
    }
  }
}

bool Predicator::precedes(const Instruction *a, const Instruction *b) const {
  if (a->getParent() == b->getParent()) {
    return a->comesBefore(b) || later.at(a->getParent()).count(a->getParent()) != 0;
  }
  return later.at(a->getParent()).count(b->getParent()) != 0;
}

const Storage *Predicator::targetOf(const Instruction *op) const {
  if (const auto *store = dyn_cast<StoreInst>(op)) {
    return analysis.access(store->getPointerOperand())->storage;
  }
  return analysis.access(cast<CallBase>(op)->getArgOperand(0))->storage;
}

bool Predicator::mutates(const Instruction *op, const Storage *storage) const { return targetOf(op) == storage; }

bool Predicator::stale(const LoadInst *load, const Instruction *op) const {
  const Storage *storage = analysis.access(load->getPointerOperand())->storage;
  for (const Instruction *other : ops) {
    if (other != op && mutates(other, storage) && precedes(load, other) && precedes(other, op)) {
      return true;
    }
  }
  return false;
}

// Bits `offset * 8` and up of the known value `known` of a `bits`-bit variable, as a value of `type`.
Value *knownPart(IRBuilder<> &b, Value *known, unsigned bits, unsigned offset, Type *type) {
  Value *whole = b.CreateZExtOrTrunc(known, b.getIntNTy(bits));
  if (offset != 0) {
    whole = b.CreateLShr(whole, 8 * offset);
  }
  return b.CreateZExtOrTrunc(whole, type);
}

Function *Predicator::copy(Instruction *op, Result result, const Value *known, std::vector<Param> &params) {
  LLVMContext &context = fn.getContext();
  const Storage *target = targetOf(op);
  Type *targetType = IntegerType::get(context, target->bits);

  // Parameters: the values the region uses from outside, and the qubit variables it reads.
  params.clear();
  std::map<const Value *, unsigned> inputIndex;
  auto addInput = [&](Value *value) {
    if (!(isa<Instruction>(value) || isa<Argument>(value)) || analysis.access(value) || inputIndex.count(value)) {
      return;
    }
    if (const auto *inst = dyn_cast<Instruction>(value); inst && inRegion.count(inst->getParent())) {
      return;
    }
    inputIndex[value] = unsigned(params.size());
    params.push_back(Param{Param::Input, value});
  };
  for (BasicBlock *block : blocks) {
    for (Instruction &inst : *block) {
      if (const auto *load = dyn_cast<LoadInst>(&inst); load && analysis.access(load->getPointerOperand())) {
        continue;
      }
      for (Use &use : inst.operands()) {
        addInput(use.get());
      }
    }
  }
  if (known) {
    addInput(const_cast<Value *>(known));
  }
  std::map<std::tuple<const Storage *, unsigned, Type *>, unsigned> currentIndex;
  auto current = [&](const Storage *storage, unsigned offset, Type *type) {
    auto key = std::make_tuple(storage, offset, type);
    auto found = currentIndex.find(key);
    if (found != currentIndex.end()) {
      return found->second;
    }
    unsigned index = unsigned(params.size());
    Param param{Param::Current};
    param.storage = storage;
    param.offset = offset;
    param.type = type;
    params.push_back(param);
    currentIndex[key] = index;
    return index;
  };
  std::map<const LoadInst *, unsigned> loadIndex;
  for (LoadInst *load : loads) {
    Access source = *analysis.access(load->getPointerOperand());
    if (stale(load, op)) {
      Param param{Param::Stale};
      param.load = load;
      loadIndex[load] = unsigned(params.size());
      params.push_back(param);
    } else if (!(known && source.storage == target)) {
      loadIndex[load] = current(source.storage, source.offset, load->getType());
    }
  }
  if (result != Result::Reached && !known) {
    current(target, 0, targetType);
  }

  std::vector<Type *> types;
  for (const Param &param : params) {
    types.push_back(param.kind == Param::Input     ? param.input->getType()
                    : param.kind == Param::Current ? param.type
                                                   : param.load->getType());
  }
  Type *returnType = result == Result::Reached ? Type::getInt1Ty(context) : targetType;
  const char *suffix = result == Result::Reached ? ".when" : result == Result::Change ? ".change" : ".update";
  Function *copy = Function::Create(FunctionType::get(returnType, types, false), GlobalValue::InternalLinkage,
                                    fn.getName() + suffix, fn.getParent());
  copy->setOnlyReadsMemory(); // the region writes no memory: outlineAround refused it
  copy->setDoesNotThrow();
  BasicBlock *top = BasicBlock::Create(context, "entry", copy);
  ValueToValueMapTy map;
  for (const auto &storage : analysis.storages) {
    map[storage->alloca] = PoisonValue::get(storage->alloca->getType()); // only used by what is removed below
  }
  for (size_t i = 0; i < params.size(); ++i) {
    if (params[i].kind == Param::Input) {
      map[params[i].input] = copy->getArg(unsigned(i));
    }
  }
  std::vector<BasicBlock *> cloned;
  for (BasicBlock *block : blocks) {
    BasicBlock *clone = CloneBasicBlock(block, map, "", copy);
    map[block] = clone;
    cloned.push_back(clone);
  }
  remapInstructionsInBlocks(cloned, map);
  auto *clonedEntry = cast<BasicBlock>(map[entry]);
  IRBuilder<> b(top);
  BranchInst *start = b.CreateBr(clonedEntry);

  // The entry's phis have one incoming edge from outside the region (run() made it so), which now comes
  // from the copy's own entry; edges from inside the region (a loop) stay.
  for (PHINode &phi : clonedEntry->phis()) {
    unsigned outside = 0;
    for (unsigned i = 0; i < phi.getNumIncomingValues(); ++i) {
      if (phi.getIncomingBlock(i)->getParent() != copy) {
        phi.setIncomingBlock(i, top);
        ++outside;
      }
    }
    if (outside != 1) {
      fail("internal: a predicated region whose entry has several edges from outside");
    }
  }
  // Edges out of the region go to the copy's exit, which returns the default (written below). This comes
  // before anything splits a block, so that every terminator is in a block of `cloned`.
  BasicBlock *out = BasicBlock::Create(context, "exit", copy);
  for (BasicBlock *clone : cloned) {
    Instruction *term = clone->getTerminator();
    for (unsigned i = 0; i < term->getNumSuccessors(); ++i) {
      if (term->getSuccessor(i)->getParent() != copy) {
        term->setSuccessor(i, out);
      }
    }
  }
  // Loads of qubit variables read the parameters, or the known value.
  b.SetInsertPoint(start);
  Value *knownCopy = known ? (isa<Constant>(known) ? const_cast<Value *>(known)
                                                   : static_cast<Value *>(copy->getArg(inputIndex.at(known))))
                           : nullptr;
  for (LoadInst *load : loads) {
    auto *clone = cast<Instruction>(map[load]);
    Value *replacement;
    auto found = loadIndex.find(load);
    if (found != loadIndex.end()) {
      replacement = copy->getArg(found->second);
    } else {
      Access source = *analysis.access(load->getPointerOperand());
      replacement = knownPart(b, knownCopy, target->bits, source.offset, load->getType());
    }
    clone->replaceAllUsesWith(replacement);
    clone->eraseFromParent();
  }

  // The operation's value, where it is reached.
  auto *opClone = cast<Instruction>(map[op]);
  BasicBlock *head = opClone->getParent();
  head->splitBasicBlock(opClone);
  head->getTerminator()->eraseFromParent();
  b.SetInsertPoint(head);
  Value *value;
  Value *variable = known ? knownPart(b, knownCopy, target->bits, 0, targetType)
                          : (result == Result::Reached ? nullptr
                                                       : copy->getArg(currentIndex.at({target, 0, targetType})));
  switch (result) {
  case Result::Reached:
    if (std::optional<OperandBundleUse> bundle = cast<CallBase>(opClone)->getOperandBundle(controlBundle)) {
      value = bundle->Inputs[0];
    } else {
      value = b.getTrue();
    }
    break;
  case Result::Change:
    value = b.CreateXor(cast<StoreInst>(opClone)->getValueOperand(), variable);
    break;
  case Result::NewValue:
    value = cast<StoreInst>(opClone)->getValueOperand();
    break;
  }
  b.CreateRet(value);

  // The region's exit returns the default.
  b.SetInsertPoint(out);
  b.CreateRet(result == Result::Reached  ? static_cast<Value *>(b.getFalse())
              : result == Result::Change ? static_cast<Value *>(ConstantInt::get(targetType, 0))
                                         : variable);

  // Quantum operations do not change ordinary values; their effect on qubit variables is in the loads.
  for (Instruction *other : ops) {
    if (auto *clone = dyn_cast_or_null<Instruction>(map.lookup(other))) {
      clone->eraseFromParent();
    }
  }
  for (BasicBlock *block : blocks) {
    for (Instruction &inst : *block) {
      if (isBitCall(&inst)) {
        cast<Instruction>(map[&inst])->eraseFromParent();
      }
    }
  }
  removeUnreachableBlocks(*copy);
  for (Instruction &inst : instructions(*copy)) {
    inst.setDebugLoc(DebugLoc()); // the copy has no debug information of its own
  }
  std::string problems;
  raw_string_ostream out2(problems);
  if (verifyFunction(*copy, &out2)) {
    fail("internal: a copy of a predicated region is broken:\n" + out2.str());
  }
  simplifyDeep(*copy);
  return copy;
}

void Predicator::refuseStale(Function *copy, const std::vector<Param> &params, const Instruction *op) const {
  for (size_t i = 0; i < params.size(); ++i) {
    if (params[i].kind == Param::Stale && !copy->getArg(unsigned(i))->use_empty()) {
      notHandled("the value of qubit variable " +
                     analysis.access(params[i].load->getPointerOperand())->storage->name +
                     " read here decides a quantum operation after the variable changed, under the control of a "
                     "value that depends on a qubit (at " +
                     locationOf(op) + ")",
                 params[i].load);
    }
  }
}

bool Predicator::readsVariable(Function *copy, const std::vector<Param> &params, const Storage *storage) const {
  for (size_t i = 0; i < params.size(); ++i) {
    if (params[i].kind == Param::Current && params[i].storage == storage && !copy->getArg(unsigned(i))->use_empty()) {
      return true;
    }
  }
  return false;
}

// The call of `copy` where the operation now is: unused parameters get an undefined value, and the qubit
// variables are read here.
CallInst *Predicator::call(Function *copy, const std::vector<Param> &params, IRBuilder<> &b) const {
  std::vector<Value *> args;
  for (size_t i = 0; i < params.size(); ++i) {
    Argument *arg = copy->getArg(unsigned(i));
    if (arg->use_empty()) {
      args.push_back(UndefValue::get(arg->getType()));
    } else if (params[i].kind == Param::Input) {
      args.push_back(params[i].input);
    } else if (params[i].kind == Param::Current) {
      Value *pointer = b.CreateConstInBoundsGEP1_64(b.getInt8Ty(), params[i].storage->alloca, params[i].offset);
      args.push_back(b.CreateLoad(params[i].type, pointer));
    } else {
      fail("internal: a stale read survived");
    }
  }
  CallInst *result = b.CreateCall(copy, args);
  // What LLVM knows of the returned bits, for the support of the variable it is stored into.
  if (auto *type = dyn_cast<IntegerType>(copy->getReturnType()); type && type->getBitWidth() > 1) {
    std::optional<KnownBits> known;
    for (BasicBlock &block : *copy) {
      if (auto *ret = dyn_cast<ReturnInst>(block.getTerminator())) {
        KnownBits bits = computeKnownBits(ret->getReturnValue(), fn.getParent()->getDataLayout());
        known = known ? known->intersectWith(bits) : bits;
      }
    }
    unsigned zeros = known ? known->countMinLeadingZeros() : 0;
    if (zeros > 0 && zeros < type->getBitWidth()) {
      Metadata *bounds[] = {
          ConstantAsMetadata::get(ConstantInt::get(type, 0)),
          ConstantAsMetadata::get(ConstantInt::get(type, APInt::getOneBitSet(type->getBitWidth(),
                                                                             type->getBitWidth() - zeros)))};
      result->setMetadata(LLVMContext::MD_range, MDNode::get(fn.getContext(), bounds));
    }
  }
  return result;
}

// The qubit `pointer` names, addressed afresh from its variable.
Value *Predicator::qubitPointer(Value *pointer, IRBuilder<> &b) const {
  Access access = *analysis.access(pointer);
  Value *variable = b.CreateConstInBoundsGEP1_64(b.getInt8Ty(), access.storage->alloca, access.offset);
  if (!isBitCall(pointer)) {
    return variable;
  }
  auto *bit = cast<CallBase>(pointer);
  return b.CreateCall(bit->getFunctionType(), bit->getCalledOperand(), {variable, bit->getArgOperand(1)});
}

// Whether `copy` returns 1 (`value`) or 0 (not `value`) whatever its arguments.
bool returnsConstant(Function *copy, bool value) {
  if (copy->size() != 1) {
    return false;
  }
  const auto *ret = dyn_cast<ReturnInst>(copy->front().getTerminator());
  const auto *constant = ret ? dyn_cast<ConstantInt>(ret->getReturnValue()) : nullptr;
  return constant && (value ? constant->isOne() : constant->isZero());
}

void Predicator::moveGate(CallBase *gate, IRBuilder<> &b) {
  const Storage *target = targetOf(gate);
  std::vector<Param> params;
  Function *when = copy(gate, Result::Reached, nullptr, params);
  refuseStale(when, params, gate);
  if (readsVariable(when, params, target)) {
    notHandled("a gate on qubit variable " + target->name + " under a condition that depends on " + target->name +
                   " (it would change the condition it is controlled by)",
               gate);
  }
  if (returnsConstant(when, false)) {
    when->eraseFromParent();
    return;
  }
  std::vector<OperandBundleDef> bundles;
  if (!returnsConstant(when, true)) {
    bundles.emplace_back(controlBundle, std::vector<Value *>{call(when, params, b)});
  } else {
    when->eraseFromParent();
  }
  std::vector<Value *> args(gate->arg_begin(), gate->arg_end());
  args[0] = qubitPointer(gate->getArgOperand(0), b);
  b.CreateCall(gate->getFunctionType(), gate->getCalledOperand(), args, bundles);
}

void Predicator::moveStore(StoreInst *store, IRBuilder<> &b) {
  const Storage *target = targetOf(store);
  Type *targetType = b.getIntNTy(target->bits);
  std::vector<Param> params;
  auto found = knownValues.find(store);
  // x ^= (where reached: new value ^ old value; elsewhere 0), when that does not read x.
  Function *change = copy(store, Result::Change, found == knownValues.end() ? nullptr : found->second, params);
  refuseStale(change, params, store);
  if (!readsVariable(change, params, target)) {
    if (returnsConstant(change, false)) {
      change->eraseFromParent(); // the store never changes the variable
      return;
    }
    CallInst *delta = call(change, params, b);
    Value *old = b.CreateLoad(targetType, target->alloca);
    b.CreateStore(b.CreateXor(old, delta), target->alloca);
    return;
  }
  change->eraseFromParent();
  // x = (where reached: new value; elsewhere x), as straight-line code for the in-place update to invert.
  Function *update = copy(store, Result::NewValue, nullptr, params);
  refuseStale(update, params, store);
  if (update->size() != 1) {
    notHandled("an update of qubit variable " + target->name +
                   " under the control of a value that depends on a qubit, which neither XORs into it nor "
                   "becomes straight-line code",
               store);
  }
  CallInst *value = call(update, params, b);
  b.CreateStore(value, target->alloca);
  InlineFunctionInfo info;
  InlineResult inlined = InlineFunction(*value, info);
  if (!inlined.isSuccess()) {
    fail(std::string("internal: cannot inline a predicated update: ") + inlined.getFailureReason());
  }
  update->eraseFromParent();
}

void Predicator::removeOperations() {
  for (Instruction *op : ops) {
    if (!isa<StoreInst>(op)) {
      op->eraseFromParent();
    }
  }
  for (Instruction *op : ops) {
    if (auto *store = dyn_cast<StoreInst>(op)) {
      store->eraseFromParent();
    }
  }
  bool changed = true;
  while (changed) {
    changed = false;
    for (BasicBlock *block : blocks) {
      for (Instruction &inst : make_early_inc_range(reverse(*block))) {
        if (isBitCall(&inst) ? inst.use_empty() : isInstructionTriviallyDead(&inst)) {
          inst.eraseFromParent();
          changed = true;
        }
      }
    }
  }
}

void Predicator::run() {
  for (BasicBlock *block : blocks) {
    for (Instruction &inst : *block) {
      if (isGateCall(&inst) || isQubitStore(inst, analysis)) {
        ops.push_back(&inst);
      }
      if (auto *load = dyn_cast<LoadInst>(&inst); load && analysis.access(load->getPointerOperand())) {
        loads.push_back(load);
      }
    }
  }
  order();
  // What is known about a store's target, before the control flow changes.
  for (Instruction *op : ops) {
    if (auto *store = dyn_cast<StoreInst>(op)) {
      Fact fact = analysis.before(store, targetOf(store));
      const auto *defined = dyn_cast_or_null<Instruction>(fact.value);
      if (fact.kind == Fact::Known && analysis.dominators.dominates(fact.value, store) &&
          !(defined && inRegion.count(defined->getParent()))) {
        knownValues[store] = fact.value;
      }
    }
  }

  // One edge into the region's entry, so that its phis have one value; one block after it, for the
  // operations.
  SetVector<BasicBlock *> outside, inside;
  for (BasicBlock *pred : predecessors(entry)) {
    if (!inRegion.count(pred)) {
      outside.insert(pred);
    }
  }
  if (outside.size() > 1) {
    SplitBlockPredecessors(entry, outside.getArrayRef(), ".predicated");
  }
  for (BasicBlock *pred : predecessors(exit)) {
    if (inRegion.count(pred)) {
      inside.insert(pred);
    }
  }
  after = SplitBlockPredecessors(exit, inside.getArrayRef(), ".after");
  if (!after) {
    fail("cannot make a block after a predicated region");
  }

  IRBuilder<> b(after->getTerminator());
  for (Instruction *op : ops) {
    b.SetCurrentDebugLocation(op->getDebugLoc());
    if (auto *store = dyn_cast<StoreInst>(op)) {
      moveStore(store, b);
    } else {
      moveGate(cast<CallBase>(op), b);
    }
  }
  for (Instruction &inst : *after) {
    if (!inst.getDebugLoc() && !ops.empty()) {
      inst.setDebugLoc(ops.front()->getDebugLoc());
    }
  }
  // Loads that read a variable after an operation of the region may have changed it.
  for (LoadInst *load : loads) {
    const Storage *storage = analysis.access(load->getPointerOperand())->storage;
    for (Instruction *op : ops) {
      if (mutates(op, storage) && precedes(op, load)) {
        readAfterChange.insert(load);
      }
    }
  }
  removeOperations();

  // What is left computes only the values the region passes on. If there are none, it goes.
  bool liveOut = !after->phis().empty();
  for (BasicBlock *block : blocks) {
    for (Instruction &inst : *block) {
      for (User *user : inst.users()) {
        liveOut = liveOut || !inRegion.count(cast<Instruction>(user)->getParent());
      }
    }
  }
  if (!liveOut) {
    for (BasicBlock *pred : SmallVector<BasicBlock *, 4>(predecessors(entry))) {
      if (!inRegion.count(pred)) {
        pred->getTerminator()->replaceSuccessorWith(entry, after);
      }
    }
    DeleteDeadBlocks(blocks);
    return;
  }
  // Otherwise the rest is outlined and its loads read the variables before the region: one that reads a
  // variable after an operation changed it would read the wrong value.
  for (BasicBlock *block : blocks) {
    for (Instruction &inst : *block) {
      if (readAfterChange.count(&inst)) {
        notHandled("the value of qubit variable " +
                       analysis.access(cast<LoadInst>(inst).getPointerOperand())->storage->name +
                       " read here, after a quantum operation under the control of a value that depends on a "
                       "qubit changed it, is used after that control ends",
                   &inst);
      }
    }
  }
}

} // namespace

bool hasQuantumOperations(const std::vector<BasicBlock *> &blocks, const ControlAnalysis &analysis) {
  for (const BasicBlock *block : blocks) {
    for (const Instruction &inst : *block) {
      if (isGateCall(&inst) || isQubitStore(inst, analysis)) {
        return true;
      }
    }
  }
  return false;
}

void predicateRegion(Function &function, const std::vector<BasicBlock *> &blocks, BasicBlock *exit,
                     ControlAnalysis &analysis) {
  Predicator(function, blocks, exit, analysis).run();
}

} // namespace bennett
