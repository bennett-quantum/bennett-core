// The control program: a function that performs quantum operations.
//
// A qubit variable becomes a register (`qalloc` where the variable is allocated, `qfree` at every return).
// Markers become gates and measurements. A value that depends on a qubit is never held in the control
// program: each store into a qubit variable outlines the whole computation of the stored value into one
// ordinary function and applies it into the variable, so no intermediate value needs a register or an
// uncomputation. `x ^= e` applies e into x. A store into a variable whose value is known (fresh, written
// with an ordinary value, or measured) first XORs the known value away. Everything else is not handled yet.
//
// Every `apply` has a fault register of its own, so that the codes of two faults can never cancel (a
// compute and its uncompute that fault on the same branch would XOR the same code twice into a shared
// register). Outside a cycle of the control flow the register is allocated on entry and released at every
// return, so the evidence stays in the state to the end; the release faults (dirty_free) if any branch
// faulted. Inside a cycle each execution allocates a fresh register and releases it right after the
// `apply`, before anything else can write it.

#include "Diagnostic.h"
#include "Emit.h"
#include "Markers.h"
#include "Normalise.h"
#include "Taint.h"

#include <llvm/Analysis/CFG.h>
#include <llvm/Analysis/ValueTracking.h>
#include <llvm/IR/Constants.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>
#include <llvm/Transforms/Utils/ValueMapper.h>

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <functional>
#include <set>

using namespace llvm;

namespace bennett {

namespace {

// The fault register is as wide as the device program's fault codes (bennett_ir::Fault::CODE_WIDTH).
constexpr unsigned faultCodeBits = 4;

// The C name of a qubit variable, for suggestions in messages.
std::string plainName(const Storage &storage) {
  return storage.alloca->hasName() ? storage.alloca->getName().str() : "x";
}

bool isZero(const Value *value) {
  const auto *constant = dyn_cast<ConstantInt>(value);
  return constant && constant->isZero();
}

// The computation of a stored value, down to its leaves.
struct Slice {
  std::vector<const Instruction *> body;     // in an order where operands come first
  std::vector<const Value *> classical;      // ordinary values: passed in
  std::vector<const Storage *> quantum;      // qubit variables read: their registers are passed in
  std::vector<const LoadInst *> loads;       // the loads of those variables
};

class ControlWriter : public BodyWriter {
public:
  ControlWriter(ModuleWriter &module, Function &function)
      : BodyWriter(module, &function), fn(function), analysis(function, module.options.taintAll) {}

  std::string write();

private:
  Function &fn;
  ControlAnalysis analysis;
  std::map<const Storage *, std::string> registers;
  // The fault registers allocated on entry and released at every return. Their `qalloc` and `qfree` lines
  // are written where the placeholders are once the whole body is known.
  std::vector<std::string> faultRegisters;
  // Blocks on a cycle of the control flow.
  std::set<const BasicBlock *> cyclic;

  void prologue() override;
  void instruction(const Instruction &inst) override;
  void terminator(const Instruction &term) override;

  void classicalUse(const Instruction &inst);
  void knownLoad(const LoadInst &load);
  void marker(const CallBase &call, const Marker &marker);
  void store(const StoreInst &store);
  void xorInto(const Storage &target, const Value *value, const Instruction &at);
  void xorClassical(const Storage &target, const Value *value, const Instruction &at);
  void xorConstant(const Storage &target, uint64_t value);
  void release(const Instruction &ret);
  // `value` at `bits` bits: a slot (converting it first if needed) or a constant, typed or not.
  std::string converted(const Value *value, unsigned bits, bool typedConstant = true);
  // Writes `apply callee(args) -> target`, with a fault register of its own, for the instruction `at`.
  void apply(const std::string &callee, const std::string &args, const Storage &target, const Instruction &at);
  Slice collect(const Value *root, const Instruction &sink);
  std::string slice(const Slice &slice, const Value *root, const Storage &target, const Instruction &sink,
                    std::string &args, bool flipProved = false);
  bool provesFlip(const Slice &slice, const Value *root, const Storage &target);
  bool sliceable(const Value *value) const;
};

// Stand-ins for the lines that allocate and release the entry fault registers.
const std::string allocatePlaceholder = "\x01allocate fault registers\n";
const std::string releasePlaceholder = "\x01release fault registers\n";

void replaceAll(std::string &text, const std::string &from, const std::string &to) {
  for (size_t at = text.find(from); at != std::string::npos; at = text.find(from, at + to.size())) {
    text.replace(at, from.size(), to);
  }
}

std::string ControlWriter::write() {
  for (const BasicBlock &block : fn) {
    for (const BasicBlock *succ : successors(&block)) {
      if (isPotentiallyReachable(succ, &block, nullptr, &analysis.dominators)) {
        cyclic.insert(&block);
        break;
      }
    }
  }
  for (const auto &storage : analysis.storages) {
    registers[storage.get()] = fresh(storage->alloca->hasName() ? storage->alloca->getName().str() : "qubits");
  }
  for (const Instruction &inst : instructions(fn)) {
    if (const auto *phi = dyn_cast<PHINode>(&inst); phi && analysis.tainted(phi)) {
      fail("internal: a phi that depends on a qubit survived outlining in @" + fn.getName().str());
    }
    // A load of a variable with a known value is that value.
    const auto *load = dyn_cast<LoadInst>(&inst);
    std::optional<Access> source = load ? analysis.access(load->getPointerOperand()) : std::nullopt;
    if (!source || analysis.isQuantumLoad(load)) {
      continue;
    }
    Fact fact = analysis.before(load, source->storage);
    unsigned width = widthOf(load->getType(), load);
    if (const auto *constant = dyn_cast<ConstantInt>(fact.value)) {
      uint64_t value = constant->getZExtValue() >> (8 * source->offset);
      alias(load, ConstantInt::get(load->getType(), value & (width == 64 ? ~uint64_t(0) : (uint64_t(1) << width) - 1)));
    } else if (source->offset == 0 && widthOf(fact.value->getType(), load) == width) {
      alias(load, fact.value);
    }
  }
  std::string text = function();
  std::string allocate, release;
  for (const std::string &faultRegister : faultRegisters) {
    allocate += "    " + faultRegister + " = qalloc " + std::to_string(faultCodeBits) + "\n";
    release += "    qfree " + faultRegister + "\n";
  }
  replaceAll(text, allocatePlaceholder, allocate);
  replaceAll(text, releasePlaceholder, release);
  return text;
}

void ControlWriter::prologue() { out += allocatePlaceholder; }

void ControlWriter::apply(const std::string &callee, const std::string &args, const Storage &target,
                          const Instruction &at) {
  std::string faultRegister = fresh(registers.at(&target).substr(1) + ".fault");
  bool once = !cyclic.count(at.getParent());
  if (once) {
    faultRegisters.push_back(faultRegister);
  } else {
    line(faultRegister + " = qalloc " + std::to_string(faultCodeBits));
  }
  line("apply " + callee + "(" + args + ") -> " + registers.at(&target) + ", " + faultRegister + " budget " +
       std::to_string(module.options.regionBudget));
  if (!once) {
    line("qfree " + faultRegister);
  }
}

void ControlWriter::instruction(const Instruction &inst) {
  if (analysis.tainted(&inst)) {
    // Computed coherently inside the functions applied where it is stored; its other uses are checked
    // where they are. A call is checked here, since it might have effects even if its result is unused.
    if (const auto *call = dyn_cast<CallBase>(&inst); call && !module.isCoherent(call->getCalledFunction())) {
      notHandled("a call that depends on a qubit, to a function that writes memory or performs quantum operations",
                 &inst);
    }
    return;
  }
  if (const auto *alloca = dyn_cast<AllocaInst>(&inst); alloca && analysis.access(alloca)) {
    const Storage *storage = analysis.access(alloca)->storage;
    line(registers.at(storage) + " = qalloc " + std::to_string(storage->bits));
    return;
  }
  if (isa<GetElementPtrInst>(inst) && analysis.access(&inst)) {
    return; // an address inside a qubit variable: resolved where it is used
  }
  if (const auto *load = dyn_cast<LoadInst>(&inst); load && analysis.access(load->getPointerOperand())) {
    knownLoad(*load);
    return;
  }
  if (const auto *storeInst = dyn_cast<StoreInst>(&inst); storeInst && analysis.access(storeInst->getPointerOperand())) {
    store(*storeInst);
    return;
  }
  if (std::optional<Marker> found = markerCall(&inst)) {
    marker(cast<CallBase>(inst), *found);
    return;
  }
  classicalUse(inst);
  BodyWriter::instruction(inst);
}

void ControlWriter::classicalUse(const Instruction &inst) {
  for (const Use &use : inst.operands()) {
    if (analysis.tainted(use.get())) {
      notHandled("a value that depends on a qubit, used by ordinary code (measure it first)", &inst);
    }
  }
}

void ControlWriter::terminator(const Instruction &term) {
  classicalUse(term);
  if (isa<ReturnInst>(term)) {
    release(term);
  }
  BodyWriter::terminator(term);
}

void ControlWriter::knownLoad(const LoadInst &load) {
  if (aliases.count(&load)) {
    return;
  }
  Access source = *analysis.access(load.getPointerOperand());
  Fact fact = analysis.before(&load, source.storage);
  unsigned from = widthOf(fact.value->getType(), &load);
  unsigned to = widthOf(load.getType(), &load);
  std::string value = operand(fact.value);
  if (source.offset != 0) {
    std::string shifted = fresh("shifted");
    line(shifted + " = lshr " + intType(from) + " " + value + ", " + std::to_string(8 * source.offset));
    value = shifted;
  }
  const char *op = to < from ? "trunc" : to > from ? "zext" : nullptr;
  if (op) {
    line(slot(&load) + " = " + op + " " + intType(from) + " " + value + " to " + intType(to));
  } else {
    line(slot(&load) + " = add " + intType(to) + " " + value + ", 0");
  }
}

void ControlWriter::marker(const CallBase &call, const Marker &marker) {
  Access target = *analysis.access(call.getArgOperand(0));
  const std::string &reg = registers.at(target.storage);
  std::string qubit = reg + "[" + std::to_string(8 * target.offset) + "]";
  switch (marker.kind) {
  case MarkerKind::Gate:
    line(marker.mnemonic + " " + qubit);
    return;
  case MarkerKind::Rotation: {
    const auto *angle = dyn_cast<ConstantFP>(call.getArgOperand(1));
    if (!angle || !std::isfinite(angle->getValueAPF().convertToDouble())) {
      notHandled("a rotation by an angle that is not a finite constant", &call);
    }
    char text[64];
    std::snprintf(text, sizeof text, "%.17g", angle->getValueAPF().convertToDouble());
    line(marker.mnemonic + "(" + text + ") " + qubit);
    return;
  }
  case MarkerKind::Measure: {
    unsigned bits = target.storage->bits;
    std::string measured = bits == 64 ? slot(&call) : fresh(reg.substr(1) + ".measured");
    line(measured + " = measure " + reg);
    if (bits < 64) {
      line(slot(&call) + " = zext " + intType(bits) + " " + measured + " to i64");
    }
    return;
  }
  }
}

void ControlWriter::store(const StoreInst &store) {
  const Storage &target = *analysis.access(store.getPointerOperand())->storage;
  const Value *value = store.getValueOperand();
  Fact fact = analysis.before(&store, &target);
  bool known = fact.kind == Fact::Known && analysis.dominators.dominates(fact.value, &store);
  // x ^= e: e is applied into x.
  if (const auto *x = dyn_cast<BinaryOperator>(value); x && x->getOpcode() == Instruction::Xor &&
                                                      analysis.tainted(value)) {
    for (unsigned side = 0; side < 2; ++side) {
      const auto *load = dyn_cast<LoadInst>(x->getOperand(side));
      if (!load || !analysis.isQuantumLoad(load)) {
        continue;
      }
      Access source = *analysis.access(load->getPointerOperand());
      if (source.storage == &target && source.offset == 0 && widthOf(load->getType(), load) == target.bits &&
          analysis.unchangedBetween(load, &store, &target)) {
        xorInto(target, x->getOperand(1 - side), store);
        return;
      }
    }
  }
  // x = v(x, rest) on a one-bit variable, where LLVM proves v(1, rest) = v(0, rest) ^ 1: that is
  // x ^= v(0, rest), however LLVM happened to write it.
  if (!known && analysis.tainted(value) && target.support == 1) {
    Slice computation = collect(value, store);
    bool readsItself = false;
    bool current = true;
    for (const LoadInst *load : computation.loads) {
      if (analysis.access(load->getPointerOperand())->storage == &target) {
        readsItself = true;
        current = current && analysis.unchangedBetween(load, &store, &target);
      }
    }
    if (readsItself && current && provesFlip(computation, value, target)) {
      std::string args;
      std::string callee = slice(computation, value, target, store, args, true);
      apply(callee, args, target, store);
      return;
    }
  }
  if (!known) {
    notHandled("an overwrite of qubit variable " + target.name +
                   ", whose value is not known here (the old value would be discarded); only `" +
                   plainName(target) + " ^= expression` is handled",
               &store);
  }
  // The variable holds a known value: XOR it away, then XOR the new value in.
  if (!analysis.tainted(value) && !(module.options.taintAll && sliceable(value))) {
    if (isa<ConstantInt>(fact.value) && isa<ConstantInt>(value)) {
      xorConstant(target, cast<ConstantInt>(fact.value)->getZExtValue() ^ cast<ConstantInt>(value)->getZExtValue());
      return;
    }
    if (isZero(fact.value) || isZero(value)) {
      xorClassical(target, isZero(value) ? fact.value : value, store);
      return;
    }
    std::string delta = fresh(target.alloca->getName().str() + ".delta");
    line(delta + " = xor " + intType(target.bits) + " " + converted(fact.value, target.bits, false) + ", " +
         converted(value, target.bits, false));
    apply(module.copyFunction(target.bits), delta, target, store);
    return;
  }
  if (!isZero(fact.value)) {
    xorClassical(target, fact.value, store);
  }
  xorInto(target, value, store);
}

void ControlWriter::xorConstant(const Storage &target, uint64_t value) {
  for (unsigned bit = 0; bit < target.bits; ++bit) {
    if (value >> bit & 1) {
      line("x " + registers.at(&target) + "[" + std::to_string(bit) + "]");
    }
  }
}

void ControlWriter::xorClassical(const Storage &target, const Value *value, const Instruction &at) {
  if (const auto *constant = dyn_cast<ConstantInt>(value)) {
    xorConstant(target, constant->getZExtValue());
    return;
  }
  apply(module.copyFunction(target.bits), converted(value, target.bits), target, at);
}

void ControlWriter::xorInto(const Storage &target, const Value *value, const Instruction &at) {
  if (isa<Constant>(value) || (!analysis.tainted(value) && !(module.options.taintAll && sliceable(value)))) {
    xorClassical(target, value, at);
    return;
  }
  std::string args;
  std::string callee = slice(collect(value, at), value, target, at, args);
  apply(callee, args, target, at);
}

std::string ControlWriter::converted(const Value *value, unsigned bits, bool typedConstant) {
  unsigned from = widthOf(value->getType(), value);
  if (isa<Constant>(resolve(value))) {
    std::string number = operand(value);
    uint64_t n = std::stoull(number);
    if (bits < 64) {
      n &= (uint64_t(1) << bits) - 1;
    }
    return (typedConstant ? intType(bits) + " " : "") + std::to_string(n);
  }
  if (from == bits) {
    return operand(value);
  }
  std::string result = fresh("converted");
  line(result + " = " + (from > bits ? "trunc " : "zext ") + intType(from) + " " + operand(value) + " to " +
       intType(bits));
  return result;
}

bool ControlWriter::sliceable(const Value *value) const {
  const auto *inst = dyn_cast<Instruction>(value);
  if (!inst || isa<PHINode>(inst) || isa<LoadInst>(inst) || isa<AllocaInst>(inst)) {
    return false;
  }
  if (const auto *call = dyn_cast<CallBase>(inst)) {
    return module.isCoherent(call->getCalledFunction());
  }
  return isa<BinaryOperator>(inst) || isa<ICmpInst>(inst) || isa<SelectInst>(inst) || isa<ZExtInst>(inst) ||
         isa<SExtInst>(inst) || isa<TruncInst>(inst) || isa<FreezeInst>(inst);
}

Slice ControlWriter::collect(const Value *root, const Instruction &sink) {
  Slice slice;
  std::set<const Value *> seen;
  std::function<void(const Value *)> visit = [&](const Value *value) {
    value = resolve(value);
    if (!seen.insert(value).second || isa<Constant>(value)) {
      return;
    }
    if (analysis.isQuantumLoad(value)) {
      const auto *load = cast<LoadInst>(value);
      slice.loads.push_back(load);
      const Storage *storage = analysis.access(load->getPointerOperand())->storage;
      if (std::find(slice.quantum.begin(), slice.quantum.end(), storage) == slice.quantum.end()) {
        slice.quantum.push_back(storage);
      }
      return;
    }
    if (!analysis.tainted(value) && !(module.options.taintAll && sliceable(value))) {
      slice.classical.push_back(value);
      return;
    }
    const auto *inst = cast<Instruction>(value);
    if (isa<PHINode>(inst)) {
      notHandled("a value that depends on a qubit, merged where ordinary control flow joins", inst);
    }
    if (const auto *load = dyn_cast<LoadInst>(inst)) {
      // Reading constant memory at an address that depends on a qubit: the applied function reads the
      // same bytes whenever it runs. Any other memory might change before the value is used.
      const auto *global = dyn_cast<GlobalVariable>(getUnderlyingObject(load->getPointerOperand()));
      if (!global || !global->isConstant() || load->isVolatile()) {
        notHandled("a load from an address that depends on a qubit, in memory that is not constant", inst);
      }
      visit(load->getPointerOperand());
    } else if (const auto *call = dyn_cast<CallBase>(inst)) {
      if (isa<IntrinsicInst>(call) || !module.isCoherent(call->getCalledFunction())) {
        notHandled("a call that depends on a qubit, to a function that writes memory or performs quantum "
                   "operations",
                   inst);
      }
      // The applied function runs where the value is stored, not where the call was: it must read the
      // same memory there.
      if (!call->getCalledFunction()->doesNotAccessMemory() && !analysis.memoryUnchangedBetween(call, &sink)) {
        notHandled("a call that depends on a qubit and reads memory that may change before its result is used",
                   inst);
      }
      for (const Use &arg : call->args()) {
        visit(arg.get());
      }
    } else if (sliceable(inst) || isa<GetElementPtrInst>(inst)) {
      for (const Use &op : inst->operands()) {
        visit(op.get());
      }
    } else {
      notHandled("the instruction `" + std::string(inst->getOpcodeName()) + "` on a value that depends on a qubit",
                 inst);
    }
    slice.body.push_back(inst);
  };
  visit(root);
  return slice;
}

// Whether LLVM's simplifier proves v(1, rest) ^ v(0, rest) = 1 for the stored value v = `root`, where the
// one-bit variable `target` is read: the computation is cloned twice into a scratch function, once with
// the variable's loads replaced by 0 and once by 1. Other qubit variables are loaded there with the
// ranges their support allows; ordinary values are arguments about which nothing is assumed.
bool ControlWriter::provesFlip(const Slice &slice, const Value *root, const Storage &target) {
  Module &module = *fn.getParent();
  LLVMContext &context = module.getContext();
  std::vector<Type *> params;
  for (const Value *value : slice.classical) {
    params.push_back(value->getType());
  }
  for (size_t i = 0; i < slice.quantum.size(); ++i) {
    params.push_back(PointerType::get(context, 0));
  }
  Function *scratch = Function::Create(FunctionType::get(Type::getInt1Ty(context), params, false),
                                       GlobalValue::InternalLinkage, "bennett.proof", module);
  BasicBlock *entry = BasicBlock::Create(context, "entry", scratch);
  IRBuilder<> b(entry);
  Value *versions[2];
  for (unsigned bit = 0; bit < 2; ++bit) {
    ValueToValueMapTy map;
    for (size_t i = 0; i < slice.classical.size(); ++i) {
      map[slice.classical[i]] = scratch->getArg(i);
    }
    for (const LoadInst *load : slice.loads) {
      Access source = *analysis.access(load->getPointerOperand());
      if (source.storage == &target) {
        map[load] = ConstantInt::get(load->getType(), source.offset == 0 ? bit : 0);
        continue;
      }
      size_t index = std::find(slice.quantum.begin(), slice.quantum.end(), source.storage) - slice.quantum.begin();
      Value *pointer = b.CreateConstInBoundsGEP1_64(b.getInt8Ty(), scratch->getArg(slice.classical.size() + index),
                                                    source.offset);
      LoadInst *copy = b.CreateLoad(load->getType(), pointer);
      copy->setMetadata(LLVMContext::MD_range, load->getMetadata(LLVMContext::MD_range));
      map[load] = copy;
    }
    for (const Instruction *inst : slice.body) {
      Instruction *copy = inst->clone();
      copy->setDebugLoc(DebugLoc());
      b.Insert(copy);
      RemapInstruction(copy, map, RF_NoModuleLevelChanges | RF_IgnoreMissingLocals);
      map[inst] = copy;
    }
    versions[bit] = map.count(root) ? static_cast<Value *>(map[root]) : const_cast<Value *>(root);
  }
  Value *flip = b.CreateXor(versions[1], versions[0]);
  b.CreateRet(b.CreateICmpEQ(flip, ConstantInt::get(flip->getType(), 1)));
  simplify(*scratch);
  const auto *ret = cast<ReturnInst>(scratch->back().getTerminator());
  const auto *proved = dyn_cast<ConstantInt>(ret->getReturnValue());
  bool holds = proved && proved->isOne();
  scratch->eraseFromParent();
  return holds;
}

// The function computing `root` from its leaves: ordinary values (passed in) and qubit variables (whose
// registers are passed in and read as integers). Every instruction of the computation dominates the sink,
// so the function computes exactly what the control program would have computed there. Loads of the
// target itself read 0: they occur only where provesFlip has shown that this gives the XOR form.
std::string ControlWriter::slice(const Slice &slice, const Value *root, const Storage &target,
                                 const Instruction &sink, std::string &args, bool flipProved) {
  bool inPlace = false;
  for (const LoadInst *load : slice.loads) {
    const Storage *storage = analysis.access(load->getPointerOperand())->storage;
    if (storage == &target) {
      inPlace = true;
      continue;
    }
    if (!analysis.unchangedBetween(load, &sink, storage)) {
      notHandled("the value of qubit variable " + storage->name + " read here is used after the variable changed (at " +
                     locationOf(&sink) + ")",
                 load);
    }
  }
  if (inPlace && !flipProved) {
    notHandled("an update of qubit variable " + target.name + " computed from its own value; only `" +
                   plainName(target) + " ^= expression` without it is handled",
               &sink);
  }

  BodyWriter writer(module, &fn);
  writer.shareAliases(*this);
  std::string params;
  for (const Value *value : slice.classical) {
    std::string name = writer.fresh(value->hasName() ? value->getName().str() : "value");
    writer.bind(value, name);
    params += (params.empty() ? "" : ", ") + intType(widthOf(value->getType(), value)) + " " + name;
    args += (args.empty() ? "" : ", ") + operand(value);
  }
  std::map<const Storage *, std::string> qubits;
  for (const Storage *storage : slice.quantum) {
    if (storage == &target) {
      continue;
    }
    std::string name = writer.fresh(storage->alloca->hasName() ? storage->alloca->getName().str() : "qubits");
    qubits[storage] = name;
    params += (params.empty() ? "" : ", ") + intType(storage->bits) + " " + name;
    args += (args.empty() ? "" : ", ") + registers.at(storage);
  }
  for (const LoadInst *load : slice.loads) {
    Access source = *analysis.access(load->getPointerOperand());
    if (source.storage == &target) {
      writer.alias(load, ConstantInt::get(load->getType(), 0));
      continue;
    }
    unsigned bits = source.storage->bits;
    unsigned width = widthOf(load->getType(), load);
    std::string value = qubits.at(source.storage);
    if (source.offset == 0 && width == bits) {
      writer.bind(load, value);
      continue;
    }
    if (source.offset != 0) {
      std::string shifted = writer.fresh("shifted");
      writer.line(shifted + " = lshr " + intType(bits) + " " + value + ", " + std::to_string(8 * source.offset));
      value = shifted;
    }
    writer.line(writer.slot(load) + " = trunc " + intType(bits) + " " + value + " to " + intType(width));
  }
  for (const Instruction *inst : slice.body) {
    writer.instruction(*inst);
  }
  if (widthOf(root->getType(), root) != target.bits) {
    fail("internal: a stored value of the wrong width");
  }
  writer.line("ret " + writer.operand(root));
  std::string text = "(" + params + ") -> " + intType(target.bits) + " {\nentry:\n" + writer.out + "}\n";
  return module.addFunction(fn.getName().str() + ".apply", text);
}

void ControlWriter::release(const Instruction &ret) {
  for (auto it = analysis.storages.rbegin(); it != analysis.storages.rend(); ++it) {
    const Storage &storage = **it;
    Fact fact = analysis.before(&ret, &storage);
    // A variable whose value is known is returned to zero first; any other is released as it is, and the
    // executor faults (dirty_free) if it is not zero on every branch.
    if (fact.kind == Fact::Known && analysis.dominators.dominates(fact.value, &ret) && !isZero(fact.value)) {
      xorClassical(storage, fact.value, ret);
    }
    line("qfree " + registers.at(&storage));
  }
  out += releasePlaceholder;
}

} // namespace

std::string writeControlFunction(ModuleWriter &module, Function &function) {
  return ControlWriter(module, function).write();
}

} // namespace bennett
