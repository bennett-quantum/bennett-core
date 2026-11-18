#include "Predicate.h"

#include "CallerPosition.h"
#include "Diagnostic.h"
#include "Markers.h"
#include "MovedLoop.h"
#include "Normalise.h"
#include "Taint.h"

#include <llvm/ADT/PostOrderIterator.h>
#include <llvm/ADT/SetVector.h>
#include <llvm/Analysis/LoopInfo.h>
#include <llvm/Analysis/ValueTracking.h>
#include <llvm/IR/Constants.h>
#include <llvm/IR/Dominators.h>
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
  return store && analysis.basedOn(store->getPointerOperand());
}

bool isBitCall(const Value *value) {
  std::optional<Marker> marker = markerCall(value);
  return marker && marker->kind == MarkerKind::Bit;
}

// What a copy of the region returns where the operation is reached, and at the region's exit.
enum class Result {
  Reached,  // i1: the operation's condition (its own, if it already has one) / 0
  Entered,  // i1: 1 where the region reaches the instruction (the first of a loop's header) / 0
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
  std::set<const Value *> beforeControl; // values defined before the original region
  std::set<const Instruction *> readAfterChange; // loads that may read what an operation wrote

  // A loop of the region whose gates stay in it: its trip count is ordinary, so the loop runs in the control
  // program, each gate controlled by the condition that the region reaches it, computed once before the loop.
  struct GateLoop {
    Loop *loop;
    std::vector<Instruction *> gates; // in the order they are moved
    bool moved = false;
  };
  std::vector<GateLoop> gateLoops;
  std::map<const Instruction *, size_t> loopOf; // a gate of a GateLoop: its index

  // Orders the blocks and operations; refuses an operation on a cycle.
  void order();
  bool precedes(const Instruction *a, const Instruction *b) const;
  bool mutates(const Instruction *op, const Storage *storage) const;
  bool stale(const LoadInst *load, const Instruction *op) const;
  const Storage *targetOf(const Instruction *op) const;
  // A copy of the region returning `result` at `op` (an operation, or for Entered any instruction that is
  // not a phi).
  Function *copy(Instruction *op, Result result, const Value *known, std::vector<Param> &params);
  void refuseStale(Function *copy, const std::vector<Param> &params, const Instruction *op) const;
  bool readsVariable(Function *copy, const std::vector<Param> &params, const Storage *storage) const;
  CallInst *call(Function *copy, const std::vector<Param> &params, IRBuilder<> &b, const Instruction &site,
                 const char *context = nullptr);
  Value *qubitPointer(Value *pointer, IRBuilder<> &b) const;
  void moveGate(CallBase *gate, IRBuilder<> &b);
  void moveStore(StoreInst *store, IRBuilder<> &b);
  // The loop of the region that contains `op`, on a cycle of the region, checked to be one the control
  // program can run (refused by name otherwise).
  size_t gateLoop(Instruction *op);
  void moveLoop(GateLoop &gates, IRBuilder<> &b);
  // The ordinary part of the condition on which the region enters `loop`, computed at `b`: the loop's own
  // guard among the tests on the way, which LLVM may have merged into the branch on a qubit.
  Value *entryGuard(Loop *loop, IRBuilder<> &b, ValueToValueMapTy &map);
  void requireSafeLoopReads();
  void requireSafeLoopReadInputs(const Value *input);
  std::set<const Value *> checkedLoopInputs;
  // The ordinary conjuncts of `branch`'s condition where it takes its true (`onTrue`) or false edge, or null
  // if it has none.
  Value *ordinaryPart(const BranchInst *branch, bool onTrue, IRBuilder<> &b, ValueToValueMapTy &map);
  // `value` at `b`, after the region: defined outside it, or an ordinary computation of the region recomputed.
  Value *recomputed(Value *value, IRBuilder<> &b, ValueToValueMapTy &map, const Instruction *at);
  // The qubit variables the computation of `value` reads, outside the region included.
  std::set<const Storage *> variablesRead(const Value *value) const;
  void removeOperations();
};

Predicator::Predicator(Function &function, const std::vector<BasicBlock *> &region, BasicBlock *exit,
                       ControlAnalysis &analysis)
    : fn(function), analysis(analysis), entry(region.front()), inRegion(region.begin(), region.end()),
      exit(exit) {
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
  // An operation on a cycle may act more than once per run of the region: a gate there stays in its loop,
  // which the control program runs (the loop's trip count must be ordinary). Loops without operations stay
  // in the copies.
  for (Instruction *op : ops) {
    if (later[op->getParent()].count(op->getParent())) {
      loopOf[op] = gateLoop(op);
      gateLoops[loopOf[op]].gates.push_back(op);
    }
  }
}

size_t Predicator::gateLoop(Instruction *op) {
  Loop *loop = analysis.loops.getLoopFor(op->getParent());
  while (loop && loop->getParentLoop() && inRegion.count(loop->getParentLoop()->getHeader())) {
    loop = loop->getParentLoop();
  }
  if (!loop || !inRegion.count(loop->getHeader())) {
    notHandled("a cycle of control flow that is not a loop, with gates inside, under the control of a value that "
               "depends on a qubit",
               op);
  }
  for (size_t i = 0; i < gateLoops.size(); ++i) {
    if (gateLoops[i].loop == loop) {
      return i;
    }
  }
  if (isQubitStore(*op, analysis)) {
    notHandled("a store into qubit variable " + targetOf(op)->name +
                   " in a loop under the control of a value that depends on a qubit, with gates in the loop (not "
                   "built yet)",
               op);
  }
  for (BasicBlock *block : loop->blocks()) {
    const Instruction *term = block->getTerminator();
    const Value *condition = isa<BranchInst>(term) && cast<BranchInst>(term)->isConditional()
                                 ? cast<BranchInst>(term)->getCondition()
                             : isa<SwitchInst>(term) ? cast<SwitchInst>(term)->getCondition()
                                                     : nullptr;
    if (condition && analysis.tainted(condition)) {
      bool exits = std::any_of(succ_begin(block), succ_end(block),
                               [&](const BasicBlock *succ) { return !loop->contains(succ); });
      if (exits) {
        notHandled("a loop with gates whose number of passes depends on a qubit: a circuit is a definite list of "
                   "gates, so the number of passes must be written as an ordinary value (a loop over an ordinary "
                   "count, testing the condition inside); such a loop does not consume its loop variable as "
                   "`while (x > 0) { ...; x--; }` does",
                   term);
      }
      notHandled("a loop with gates whose passes decide on values that depend on a qubit (not built yet: one "
                 "applied function returning a table of control bits for all passes)",
                 term);
    }
    for (const Instruction &inst : *block) {
      if (analysis.tainted(&inst)) {
        notHandled("a loop with gates that computes values that depend on a qubit in its passes (not built yet: "
                   "one applied function returning a table of control bits for all passes)",
                   &inst);
      }
      if (isQubitStore(inst, analysis)) {
        notHandled("a store into qubit variable " + targetOf(&inst)->name +
                       " in a loop under the control of a value that depends on a qubit, with gates in the loop "
                       "(not built yet)",
                   &inst);
      }
      if (isGateCall(&inst)) {
        if (cast<CallBase>(inst).getOperandBundle(controlBundle)) {
          fail("internal: a controlled gate in a loop under a qubit-dependent condition");
        }
      } else if (!isBitCall(&inst)) {
        requireSafeMovedLoopInstruction(inst);
      }
      for (const User *user : inst.users()) {
        if (!loop->contains(cast<Instruction>(user))) {
          notHandled("a loop with gates under the control of a value that depends on a qubit, whose values are "
                     "used after it (not built yet)",
                     &inst);
        }
      }
    }
  }
  if (!loop->getLoopPredecessor()) {
    notHandled("a loop with gates under the control of a value that depends on a qubit, entered from more than "
               "one place",
               op);
  }
  gateLoops.push_back(GateLoop{loop, {}});
  return gateLoops.size() - 1;
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
  bool condition = result == Result::Reached || result == Result::Entered;
  const Storage *target = condition ? nullptr : targetOf(op);
  Type *targetType = target ? IntegerType::get(context, target->bits) : nullptr;

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
  if (!condition && !known) {
    current(target, 0, targetType);
  }

  std::vector<Type *> types;
  for (const Param &param : params) {
    types.push_back(param.kind == Param::Input     ? param.input->getType()
                    : param.kind == Param::Current ? param.type
                                                   : param.load->getType());
  }
  Type *returnType = condition ? Type::getInt1Ty(context) : targetType;
  const char *suffix = condition ? ".when" : result == Result::Change ? ".change" : ".update";
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
  Value *variable = known       ? knownPart(b, knownCopy, target->bits, 0, targetType)
                    : condition ? nullptr
                                : copy->getArg(currentIndex.at({target, 0, targetType}));
  switch (result) {
  case Result::Reached:
    if (std::optional<OperandBundleUse> bundle = cast<CallBase>(opClone)->getOperandBundle(controlBundle)) {
      value = bundle->Inputs[0];
    } else {
      value = b.getTrue();
    }
    break;
  case Result::Entered:
    value = b.getTrue();
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
  b.CreateRet(condition                  ? static_cast<Value *>(b.getFalse())
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
CallInst *Predicator::call(Function *copy, const std::vector<Param> &params, IRBuilder<> &b,
                          const Instruction &site, const char *context) {
  requireCallerIndependent(*copy, site, context);
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
    bundles.emplace_back(controlBundle, std::vector<Value *>{call(when, params, b, *gate)});
  } else {
    when->eraseFromParent();
  }
  std::vector<Value *> args(gate->arg_begin(), gate->arg_end());
  args[0] = qubitPointer(gate->getArgOperand(0), b);
  b.CreateCall(gate->getFunctionType(), gate->getCalledOperand(), args, bundles);
}

Value *Predicator::recomputed(Value *value, IRBuilder<> &b, ValueToValueMapTy &map, const Instruction *at) {
  auto *inst = dyn_cast<Instruction>(value);
  if (!inst || !inRegion.count(inst->getParent()) || analysis.access(inst) || isBitCall(inst)) {
    return value; // defined before the region (so before the loop's new place), or a qubit, addressed afresh
  }
  if (Value *done = map.lookup(inst)) {
    return done;
  }
  if (analysis.tainted(inst) || isa<PHINode>(inst) || inst->isTerminator()) {
    notHandled("a loop with gates under the control of a value that depends on a qubit, using a value computed "
               "under that control before the loop (not built yet)",
               at);
  }
  requireSafeMovedLoopInstruction(*inst);
  Instruction *copy = inst->clone();
  for (Use &use : copy->operands()) {
    use.set(recomputed(use.get(), b, map, at));
  }
  b.Insert(copy);
  map[inst] = copy;
  return copy;
}

bool readsImmutableMemory(const LoadInst &load) {
  const DataLayout &layout = load.getModule()->getDataLayout();
  int64_t offset;
  const auto *global = dyn_cast<GlobalVariable>(GetPointerBaseWithConstantOffset(load.getPointerOperand(), offset,
                                                                              layout));
  if (!global || global->isDeclaration() || !global->isConstant() || offset < 0) {
    return false;
  }
  TypeSize object = layout.getTypeAllocSize(global->getValueType());
  TypeSize access = layout.getTypeStoreSize(load.getType());
  return !object.isScalable() && !access.isScalable() && uint64_t(offset) <= object.getFixedValue() &&
         access.getFixedValue() <= object.getFixedValue() - uint64_t(offset);
}

void Predicator::requireSafeLoopReadInputs(const Value *input) {
  SmallVector<const Value *, 8> work{input};
  while (!work.empty()) {
    const auto *inst = dyn_cast<Instruction>(work.pop_back_val());
    if (!inst || !checkedLoopInputs.insert(inst).second) {
      continue;
    }
    // A tracked qubit variable is read as a register. An ordinary load can
    // have been speculated by LLVM, even when it now dominates the region.
    if (const auto *load = dyn_cast<LoadInst>(inst)) {
      if (!analysis.access(load->getPointerOperand()) && !readsImmutableMemory(*load)) {
        notHandled("a moved gate loop may read bytes not proved ordinary before its gates "
                   "(route: keep the conditional read inside one applied function for the table of control bits)",
                   load);
      }
    } else if (const auto *call = dyn_cast<CallBase>(inst);
               call && !markerCall(call) && call->mayReadFromMemory()) {
      notHandled("a moved gate loop may call a function that reads bytes not proved ordinary before its gates "
                 "(route: keep the conditional read inside one applied function for the table of control bits)",
                 call);
    }
    for (const Use &operand : inst->operands()) {
      work.push_back(operand.get());
    }
  }
}

void Predicator::requireSafeLoopReads() {
  // Check both sides of a test in the prefix, even when they rejoin before the
  // loop. Follow inputs to their definitions, including loads outside the region.
  for (BasicBlock *block : blocks) {
    if (any_of(gateLoops, [&](const GateLoop &gates) {
          return gates.loop->contains(block) || later.at(block).count(gates.loop->getHeader());
        })) {
      for (const Instruction &inst : *block) {
        requireSafeLoopReadInputs(&inst);
      }
    }
  }
}

Value *Predicator::entryGuard(Loop *loop, IRBuilder<> &b, ValueToValueMapTy &map) {
  // Every test on the way into the loop decides it: each conditional branch, from the loop's predecessor up
  // through the region to the branch that enters the region, one of whose edges every path to the header
  // takes. The loop's own guard is among them, but not always in the predecessor: LLVM may have hoisted code
  // into a preheader of its own (a nested loop's guard) or merged the guard into the branch on the qubit.
  DominatorTree dominators(fn); // the region's blocks are as they were; only code after it is new
  BasicBlock *header = loop->getHeader();
  Value *guard = nullptr;
  for (DomTreeNode *node = dominators.getNode(loop->getLoopPredecessor()); node; node = node->getIDom()) {
    BasicBlock *from = node->getBlock();
    const Instruction *term = from->getTerminator();
    if (const auto *choice = dyn_cast<SwitchInst>(term); choice && !analysis.tainted(choice->getCondition())) {
      notHandled("a loop with gates under the control of a value that depends on a qubit, entered by a switch",
                 term);
    }
    const auto *branch = dyn_cast<BranchInst>(term);
    if (branch && branch->isConditional() && branch->getSuccessor(0) != branch->getSuccessor(1)) {
      for (bool onTrue : {true, false}) {
        if (dominators.dominates(BasicBlockEdge(from, branch->getSuccessor(onTrue ? 0 : 1)), header)) {
          if (!inRegion.count(from)) {
            requireSafeLoopReadInputs(branch->getCondition());
          }
          if (Value *part = ordinaryPart(branch, onTrue, b, map)) {
            guard = guard ? b.CreateAnd(guard, part) : part;
          }
        }
      }
    }
    if (!inRegion.count(from)) {
      break; // the block whose branch enters the region
    }
  }
  return guard ? guard : b.getTrue();
}

Value *Predicator::ordinaryPart(const BranchInst *branch, bool onTrue, IRBuilder<> &b, ValueToValueMapTy &map) {
  // Where the condition holds (fails), its ordinary conjuncts (disjuncts) do too.
  unsigned opcode = onTrue ? Instruction::And : Instruction::Or;
  Value *guard = nullptr;
  std::vector<Value *> work{branch->getCondition()};
  while (!work.empty()) {
    Value *value = work.back();
    work.pop_back();
    auto *binary = dyn_cast<BinaryOperator>(value);
    auto *select = dyn_cast<SelectInst>(value);
    if (binary && binary->getOpcode() == opcode) {
      work.push_back(binary->getOperand(0));
      work.push_back(binary->getOperand(1));
    } else if (select && onTrue && isa<ConstantInt>(select->getFalseValue()) &&
               cast<ConstantInt>(select->getFalseValue())->isZero()) {
      work.push_back(select->getCondition()); // a && b written as select a, b, false
      work.push_back(select->getTrueValue());
    } else if (select && !onTrue && isa<ConstantInt>(select->getTrueValue()) &&
               cast<ConstantInt>(select->getTrueValue())->isOne()) {
      work.push_back(select->getCondition()); // a || b written as select a, true, b
      work.push_back(select->getFalseValue());
    } else if (!analysis.tainted(value)) {
      Value *term = recomputed(value, b, map, branch);
      term = onTrue ? term : b.CreateNot(term);
      guard = guard ? b.CreateAnd(guard, term) : term;
    }
  }
  return guard;
}

std::set<const Storage *> Predicator::variablesRead(const Value *value) const {
  std::set<const Storage *> read;
  std::set<const Value *> seen;
  std::vector<const Value *> work{value};
  while (!work.empty()) {
    const Value *next = work.back();
    work.pop_back();
    if (!analysis.tainted(next) || !seen.insert(next).second) {
      continue;
    }
    if (analysis.isQuantumLoad(next)) {
      read.insert(analysis.access(cast<LoadInst>(next)->getPointerOperand())->storage);
      continue;
    }
    for (const Use &use : cast<User>(next)->operands()) {
      work.push_back(use.get());
    }
  }
  return read;
}

// Whether `copy` returns one qubit of a variable as it is (the condition `if (q)` on a _Bool): the control
// program then makes it the gates' control qubit (Control.cpp, literals) without computing anything.
bool returnsOneQubit(Function *copy, const std::vector<Param> &params) {
  if (copy->size() != 1) {
    return false;
  }
  const auto *ret = dyn_cast<ReturnInst>(copy->front().getTerminator());
  const Value *value = ret ? ret->getReturnValue() : nullptr;
  for (;;) {
    if (const auto *compare = dyn_cast_or_null<ICmpInst>(value);
        compare && compare->getPredicate() == ICmpInst::ICMP_NE && isa<ConstantInt>(compare->getOperand(1)) &&
        cast<ConstantInt>(compare->getOperand(1))->isZero()) {
      value = compare->getOperand(0);
    } else if (const auto *cast = dyn_cast_or_null<CastInst>(value); cast && (isa<TruncInst>(cast) || isa<ZExtInst>(cast))) {
      value = cast->getOperand(0);
    } else {
      break;
    }
  }
  const auto *arg = dyn_cast_or_null<Argument>(value);
  if (!arg) {
    return false;
  }
  const Param &param = params[arg->getArgNo()];
  return param.kind == Param::Current && param.offset == 0 && popcount(param.storage->support) == 1;
}

// The loop runs in the control program after the region, as written: its trip count is ordinary. Its gates
// act where the region reaches the loop: one condition, computed once before the loop into a fresh qubit
// variable (or one qubit as it is), controls every gate of the loop in every pass, and is computed again after
// the loop, returning it to 0. The condition is that the region reaches the loop's header, not a gate: nothing
// in the loop depends on a qubit (gateLoop refused it), so which gates act in which pass is decided by the
// loop's own ordinary code, which the control program runs, and the condition's `apply` runs only the path to
// the loop, none of its passes. The gates must not change what the condition reads. No loop is unrolled or
// created; the loop also runs for the configurations whose path would not reach it, as ordinary code that
// writes no memory (outlineAround refused writes), its gates acting on none of them.
void Predicator::moveLoop(GateLoop &gates, IRBuilder<> &b) {
  LLVMContext &context = fn.getContext();
  Loop *loop = gates.loop;
  BasicBlock *pre = b.GetInsertBlock();
  BasicBlock *cont = pre->splitBasicBlock(b.GetInsertPoint(), pre->getName() + ".loop");
  b.SetInsertPoint(pre->getTerminator());

  // The condition, before the loop.
  enum class Kind { Never, Always, Qubit, Variable };
  Kind kind = Kind::Variable;
  Value *value = nullptr;      // Qubit: the copy's result, for the control program's literal controls
  AllocaInst *qubit = nullptr; // Variable: the fresh qubit variable holding it
  std::vector<Param> params;
  Function *when = copy(loop->getHeader()->getFirstNonPHI(), Result::Entered, nullptr, params);
  refuseStale(when, params, gates.gates.front());
  std::set<const Storage *> inputs;
  for (size_t i = 0; i < params.size(); ++i) {
    if (params[i].kind == Param::Input && !when->getArg(unsigned(i))->use_empty()) {
      std::set<const Storage *> read = variablesRead(params[i].input);
      inputs.insert(read.begin(), read.end());
    }
  }
  for (Instruction *gate : gates.gates) {
    if (readsVariable(when, params, targetOf(gate)) || inputs.count(targetOf(gate))) {
      notHandled("a gate on qubit variable " + targetOf(gate)->name +
                     " in a loop whose gates are controlled by a condition that reads it (the gate would change "
                     "the condition; not built yet)",
                 gate);
    }
  }
  const char *conditionContext = " when evaluating a moved gate-loop condition";
  if (returnsConstant(when, false)) {
    kind = Kind::Never;
  } else if (returnsConstant(when, true)) {
    kind = Kind::Always;
  } else if (returnsOneQubit(when, params)) {
    kind = Kind::Qubit;
    value = call(when, params, b, *gates.gates.front(), conditionContext);
  } else {
    IRBuilder<> top(&*fn.getEntryBlock().getFirstInsertionPt());
    qubit = top.CreateAlloca(b.getInt8Ty(), nullptr, "condition");
    qubit->setMetadata(qubitMetadata, MDNode::get(context, {}));
    Value *old = b.CreateLoad(b.getInt8Ty(), qubit);
    Value *enabled = call(when, params, b, *gates.gates.front(), conditionContext);
    b.CreateStore(b.CreateXor(old, b.CreateZExt(enabled, b.getInt8Ty())), qubit);
  }

  // The loop, cloned between `pre` and `cont`, entered where its ordinary guard holds; what it uses from the
  // region is recomputed in `pre` (ordinary values: the region writes no memory).
  ValueToValueMapTy map;
  for (BasicBlock *block : loop->blocks()) {
    for (Instruction &inst : *block) {
      for (Use &use : inst.operands()) {
        if (auto *operand = dyn_cast<Instruction>(use.get()); operand && !loop->contains(operand)) {
          recomputed(operand, b, map, &inst);
        }
      }
    }
  }
  Value *guard = entryGuard(loop, b, map);
  std::vector<BasicBlock *> clones;
  std::set<const BasicBlock *> cloned;
  for (BasicBlock *block : loop->blocks()) {
    BasicBlock *clone = CloneBasicBlock(block, map, ".gates", &fn);
    map[block] = clone;
    clones.push_back(clone);
    cloned.insert(clone);
  }
  remapInstructionsInBlocks(clones, map);
  auto *header = cast<BasicBlock>(map[loop->getHeader()]);
  pre->getTerminator()->eraseFromParent();
  b.SetInsertPoint(pre);
  b.CreateCondBr(guard, header, cont);
  for (BasicBlock *clone : clones) {
    Instruction *term = clone->getTerminator();
    for (unsigned i = 0; i < term->getNumSuccessors(); ++i) {
      if (!cloned.count(term->getSuccessor(i))) {
        term->setSuccessor(i, cont);
      }
    }
  }
  for (PHINode &phi : header->phis()) {
    for (unsigned i = 0; i < phi.getNumIncomingValues(); ++i) {
      if (!cloned.count(phi.getIncomingBlock(i))) {
        phi.setIncomingBlock(i, pre);
      }
    }
  }
  requireFiniteMovedLoop(*header, *gates.gates.front(), beforeControl);
  for (Instruction *gate : gates.gates) {
    auto *clone = cast<CallBase>(map[gate]);
    if (kind == Kind::Never) {
      clone->eraseFromParent();
      continue;
    }
    IRBuilder<> g(clone);
    g.SetCurrentDebugLocation(gate->getDebugLoc());
    std::vector<Value *> args(clone->arg_begin(), clone->arg_end());
    args[0] = qubitPointer(gate->getOperand(0), g);
    std::vector<OperandBundleDef> bundles;
    if (kind == Kind::Qubit) {
      bundles.emplace_back(controlBundle, std::vector<Value *>{value});
    } else if (kind == Kind::Variable) {
      Value *set = g.CreateICmpNE(g.CreateLoad(g.getInt8Ty(), qubit), g.getInt8(0));
      bundles.emplace_back(controlBundle, std::vector<Value *>{set});
    }
    g.CreateCall(clone->getFunctionType(), clone->getCalledOperand(), args, bundles);
    clone->eraseFromParent();
  }

  // The condition again, after the loop: it returns its qubit variable to 0.
  b.SetInsertPoint(cont->getTerminator());
  if (kind == Kind::Variable) {
    Value *old = b.CreateLoad(b.getInt8Ty(), qubit);
    Value *enabled = call(when, params, b, *gates.gates.front(), conditionContext);
    b.CreateStore(b.CreateXor(old, b.CreateZExt(enabled, b.getInt8Ty())), qubit);
  } else if (kind != Kind::Qubit && when->use_empty()) {
    when->eraseFromParent();
  }
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
    CallInst *delta = call(change, params, b, *store);
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
  CallInst *value = call(update, params, b, *store);
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
  if (!gateLoops.empty()) {
    requireSafeLoopReads();
    for (const Instruction &inst : instructions(fn)) {
      if (!inRegion.count(inst.getParent()) && analysis.dominators.dominates(&inst, &entry->front())) {
        beforeControl.insert(&inst);
      }
    }
  }
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
    if (auto found = loopOf.find(op); found != loopOf.end()) {
      GateLoop &gates = gateLoops[found->second];
      if (!gates.moved) {
        moveLoop(gates, b);
        gates.moved = true;
      }
    } else if (auto *store = dyn_cast<StoreInst>(op)) {
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
