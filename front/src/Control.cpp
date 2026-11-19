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
// return, so the evidence stays in the state to the end. Inside a cycle each execution allocates a fresh
// register and releases it right after the `apply`, before anything else can write it. A release is the
// partial trace: a fault register that holds a code is dropped as it is, which ends nothing; the program
// cannot see the fault unless it measures the register first. The simulator stops by default at the faulting
// `apply` itself (a region fault) and reports a release of a register that was not zero, both its own
// diagnostics.

#include "Applied.h"
#include "Classify.h"
#include "Diagnostic.h"
#include "Emit.h"
#include "Evaluations.h"
#include "HeldBytes.h"
#include "Markers.h"
#include "Passes.h"
#include "RegionResults.h"
#include "Taint.h"

#include <llvm/ADT/StringExtras.h>
#include <llvm/Analysis/CFG.h>
#include <llvm/Analysis/ValueTracking.h>
#include <llvm/IR/Constants.h>
#include <llvm/IR/Function.h>
#include <llvm/IR/InstIterator.h>
#include <llvm/IR/Instructions.h>
#include <llvm/IR/IRBuilder.h>
#include <llvm/IR/IntrinsicInst.h>
#include <llvm/IR/Module.h>
#include <llvm/Transforms/Utils/Cloning.h>
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
constexpr unsigned faultCodeBits = 5;

// The most support bits of the other qubit variables a one-bit update reads for which provesFlip checks
// every assignment when LLVM's simplifier cannot fold the identity: two evaluations per assignment, each a
// clone of the computation folded by LLVM, so 2^9 folds at most.
constexpr unsigned maxEnumeratedSupportBits = 8;

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
  // Loads of qubit variables that held a known ordinary value where they read it (`collect` with `knownLoads`):
  // that value is passed in for them.
  std::vector<std::pair<const LoadInst *, const Value *>> known;
  // Loads of qubit variables that held a value computed from registers unchanged since (Fact::Computed), with
  // `knownLoads`: that computation, in the body, stands for them.
  std::vector<std::pair<const LoadInst *, const Value *>> computed;
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
  void exchange(const CallBase &call);
  void knownLoad(const LoadInst &load);
  void marker(const CallBase &call, const Marker &marker);
  void measureBytes(const CallBase &call);
  void store(const StoreInst &store);
  void xorInto(const Storage &target, const Value *value, const Instruction &at);
  void xorClassical(const Storage &target, const Value *value, const Instruction &at);
  void xorConstant(const Storage &target, uint64_t value);
  // An overwrite of `target` whose old value the front cannot compute back: the new value is applied into a
  // fresh register, exchanged with the variable by CNOTs, and the register, now holding the old value, is
  // released (the partial trace), with a warning.
  void retainedOverwrite(const Storage &target, const Value *value, const StoreInst &store);
  // The stores of a region's exchanged variables (RegionResults.h), from `first` on: stores by exchange.
  void exchangedStores(const StoreInst &first, const MDNode &group);
  std::set<const StoreInst *> exchangedDone;
  // Whether `old`, the value `target` holds at `store`, can be computed again there: a computation the control
  // program can apply (collect) whose qubit variables, `target` not among them, are unchanged since they were read
  // or held there a value that can be computed again in turn (collect with knownLoads computes it in their place),
  // and whose calls see the same memory.
  bool recomputable(const Value *old, const Storage &target, const Instruction &store) const {
    std::set<const Value *> active;
    return recomputable(old, target, store, active);
  }
  // The same, with the values whose computations are being checked (`active`), so that a load recorded as
  // holding a value computed from itself is not followed in a cycle.
  bool recomputable(const Value *old, const Storage &target, const Instruction &store,
                    std::set<const Value *> &active) const;

  void release(const Instruction &ret);
  void controlledGate(const CallBase &call, const Access &target, const std::string &gate);
  // The qubits whose conjunction `condition` is, if LLVM's simplifier proves it is one: each qubit of a
  // variable whose support is that one bit, and whether it must be 1 (or 0).
  std::optional<std::vector<std::pair<std::string, bool>>> literals(const Slice &slice, const Value *condition);
  // `root` with each variable in `assigned` replaced by the value given, folded by LLVM's simplifier with
  // every call it makes inlined once, if that gives a constant.
  std::optional<uint64_t> evaluate(const Slice &slice, const Value *root,
                                   const std::map<const Storage *, uint64_t> &assigned);
  // `value` at `bits` bits: a slot (converting it first if needed) or a constant, typed or not.
  std::string converted(const Value *value, unsigned bits, bool typedConstant = true);
  // Writes `apply callee(args) -> target`, with a fault register of its own, for the instruction `at`.
  void apply(const std::string &callee, const std::string &args, const std::string &target, const Instruction &at);
  void apply(const std::string &callee, const std::string &args, const Storage &target, const Instruction &at) {
    apply(callee, args, registers.at(&target), at);
  }
  // The computation of `root` at `sink`. With `knownLoads`, a load of a qubit variable that held a known ordinary
  // value where it read it (knownAt) reads that value, and one that held a value it can compute again at `sink`
  // (recomputable) reads that computation.
  Slice collect(const Value *root, const Instruction &sink, bool knownLoads = false);
  // The ordinary value `load` read, if its variable held one there that is still the same value at `sink`.
  const Value *knownAt(const LoadInst &load, const Instruction &sink) const;
  // The function computing `root` at `sink` into a register of `bits` bits: the variable `target`, or a
  // fresh register when it is null.
  std::string slice(const Slice &slice, const Value *root, const Storage *target, unsigned bits,
                    const Instruction &sink, std::string &args, bool flipProved = false);
  // Refuses by name a load of the slice, other than of `target`, whose variable may change between the load
  // and `sink`: what is computed at `sink` reads the registers as they are there, not as they were read.
  void requireCurrent(const Slice &slice, const Storage *target, const Instruction &sink);
  bool provesFlip(const Slice &slice, const Value *root, const Storage &target);
  // provesFlip by evaluating both values of the target on every assignment of the other variables' support
  // bits, when the computation has no ordinary leaves and those bits are few.
  bool provesFlipByCases(const Slice &slice, const Value *root, const Storage &target);
  bool sliceable(const Value *value) const;
  // Refuses by name a call with qubit-dependent arguments that cannot be applied whole (Applied.h).
  void requireApplicable(const CallBase &call) const;
  // Refuses by name an applied call whose result is not used, to a function that may write memory.
  void requireUsedResult(const CallBase &call) const;
  // Whether `call` may read or write memory outside the frames of the functions it calls (Aliasing.h). Only
  // such memory can change between the call and where its result is used, and only there can another call
  // of the same computation see what it writes.
  bool sharesMemory(const CallBase &call) const {
    return !call.getCalledFunction()->doesNotAccessMemory() && module.aliasing().reachesOutsideFrames(fn, call);
  }

  // Evaluations (Evaluations.h): those each generated function computes, where each function is applied, and an
  // instruction of each, for messages.
  std::map<std::string, std::set<const MDNode *>> evaluated;
  // Those of them that depend on a qubit (an ordinary call `--taint-all` copies into a slice runs as ordinary code
  // too, and is not a value with several consumers).
  std::set<const MDNode *> qubitEvaluations;
  std::map<std::string, std::vector<const Instruction *>> appliedAt;
  std::map<const MDNode *, const Instruction *> evaluationSite;
  // Why an evaluation with several consumers has no register of its own (Evaluations.h), where it was recorded.
  std::map<const MDNode *, std::string> withoutRegister;
  // Those that have one.
  std::set<const MDNode *> registered;
  void noteEvaluation(const MDNode *evaluation, const Instruction &inst) {
    evaluationSite.emplace(evaluation, &inst);
    if (inst.getMetadata(sharedCallMetadata)) {
      registered.insert(evaluation);
    }
    if (const MDNode *why = inst.getMetadata(sharedRefusedMetadata)) {
      withoutRegister.emplace(evaluation, cast<MDString>(why->getOperand(0))->getString().str());
    }
  }
  // The evaluations of a slice's body, refusing by name a frame observation that moving them would change.
  std::set<const MDNode *> sliceEvaluations(const Slice &slice);
  // Refuses by name an evaluation computed by more than one generated function or also by ordinary code, and a
  // generated function with evaluations applied at two places across a reservation or release of the stack.
  std::set<const MDNode *> ordinaryEvaluations;
  void requireSingleEvaluations() const;
  void requireSameStack(const std::vector<const Instruction *> &sites, const MDNode &evaluation) const;
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
  requireSingleEvaluations();
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

void ControlWriter::apply(const std::string &callee, const std::string &args, const std::string &target,
                          const Instruction &at) {
  appliedAt[callee].push_back(&at);
  std::string faultRegister = fresh(target.substr(1) + ".fault");
  bool once = !cyclic.count(at.getParent());
  if (once) {
    faultRegisters.push_back(faultRegister);
  } else {
    line(faultRegister + " = qalloc " + std::to_string(faultCodeBits));
  }
  line("apply " + callee + "(" + args + ") -> " + target + ", " + faultRegister + " budget " +
       std::to_string(module.options.regionBudget));
  if (!once) {
    line("qfree " + faultRegister);
  }
}

void ControlWriter::instruction(const Instruction &inst) {
  if (isHolds(&inst)) {
    return; // a record for the analysis (Taint.h)
  }
  if (const MDNode *evaluation = evaluationOf(inst); evaluation && !analysis.tainted(&inst)) {
    ordinaryEvaluations.insert(evaluation);
    noteEvaluation(evaluation, inst);
  }
  if (isExchange(&inst)) {
    exchange(cast<CallBase>(inst));
    return;
  }
  if (analysis.tainted(&inst)) {
    // Computed coherently inside the functions applied where it is stored; its other uses are checked
    // where they are. What has an effect is checked here, since the effect would otherwise vanish with an
    // unused result: a call, and any other instruction with an effect on memory (an atomic update, a
    // volatile or atomic read). A load of a qubit variable reads its register, never memory.
    const auto *load = dyn_cast<LoadInst>(&inst);
    bool readsRegister = load && analysis.access(load->getPointerOperand());
    if (const auto *call = dyn_cast<CallBase>(&inst)) {
      requireApplicable(*call);
      requireUsedResult(*call);
      if (!call->onlyReadsMemory()) {
        module.appliedCalls().warnUndoneWrites(*call, analysis.dominators);
      }
    } else if (!readsRegister && inst.mayHaveSideEffects()) {
      notHandled("an effect on memory (the instruction `" + std::string(inst.getOpcodeName()) +
                     "`) that depends on a qubit",
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
  // A call with arguments that depend on a qubit whose result LLVM removed (it was not used): the call has no
  // value to depend on a qubit, but it is still an applied call. One with a pointer argument may return its
  // result through memory (a wide aggregate or a vector, Abi.h), so its result may be used.
  if (const auto *call = dyn_cast<CallBase>(&inst);
      call && !isa<IntrinsicInst>(call) && module.isApplicable(call->getCalledFunction()) &&
      std::any_of(call->arg_begin(), call->arg_end(), [&](const Use &arg) { return analysis.tainted(arg.get()); }) &&
      std::none_of(call->arg_begin(), call->arg_end(), [](const Use &arg) { return arg->getType()->isPointerTy(); })) {
    requireUsedResult(*call);
  }
  // Translated as an ordinary call from here on: a call classified before the optimiser as applied must not be
  // (Classify.h).
  if (const auto *call = dyn_cast<CallBase>(&inst)) {
    requireAppliedCallKept(*call, std::any_of(call->arg_begin(), call->arg_end(), [&](const Use &arg) {
                             return analysis.tainted(arg.get());
                           }));
    if (!markerCall(call) && !isa<IntrinsicInst>(call)) {
      module.noteOrdinaryCall(*call);
    }
  }
  classicalUse(inst);
  BodyWriter::instruction(inst);
}

void ControlWriter::classicalUse(const Instruction &inst) {
  for (const Use &use : inst.operands()) {
    if (!analysis.tainted(use.get())) {
      continue;
    }
    // Read from memory that holds qubits: the measurement the program needs is of those bytes.
    Slice computation = collect(use.get(), inst);
    std::vector<const Storage *> read(computation.quantum.begin(), computation.quantum.end());
    for (const Value *leaf : computation.classical) {
      if (const auto *load = dyn_cast<LoadInst>(leaf); load && analysis.basedOn(load->getPointerOperand())) {
        read.push_back(analysis.basedOn(load->getPointerOperand()));
      }
    }
    for (const Storage *storage : read) {
      if (storage->held) {
        notHandled("a value read from memory that holds qubits (qubit variable " + storage->name +
                       "), used by ordinary code (measure the bytes first, with bennett_measure_bytes)",
                   &inst);
      }
    }
    notHandled("a value that depends on a qubit, used by ordinary code (measure it first)", &inst);
  }
}

// Three CNOTs per bit exchange the shadow's register and the bits of memory from the address
// (docs/DESIGN.md, "Memory written under a qubit"); the first gate at a byte makes it hold qubits.
void ControlWriter::exchange(const CallBase &call) {
  const Storage &shadow = *analysis.access(call.getArgOperand(0))->storage;
  std::string address = operand(call.getArgOperand(1));
  for (unsigned bit = 0; bit < shadow.bits; ++bit) {
    std::string held = registers.at(&shadow) + "[" + std::to_string(bit) + "]";
    std::string memory = "[" + address + ":" + std::to_string(bit) + "]";
    line("x " + held + " ctrl " + memory);
    line("x " + memory + " ctrl " + held);
    line("x " + held + " ctrl " + memory);
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
  if (marker.kind == MarkerKind::Bit) {
    return; // a qubit, named where a gate uses it
  }
  if (marker.kind == MarkerKind::MeasureBytes && !analysis.access(call.getArgOperand(0))) {
    measureBytes(call);
    return;
  }
  Access target = *analysis.access(call.getArgOperand(0));
  const std::string &reg = registers.at(target.storage);
  std::string qubit = reg + "[" + std::to_string(target.bit) + "]";
  switch (marker.kind) {
  case MarkerKind::Bit:
    return;
  case MarkerKind::Gate:
    controlledGate(call, target, marker.mnemonic + " " + qubit);
    return;
  case MarkerKind::Rotation: {
    std::optional<double> angle = rotationAngle(call);
    if (!angle || !std::isfinite(*angle)) {
      notHandled("a rotation by an angle that is not a finite constant", &call);
    }
    char text[64];
    std::snprintf(text, sizeof text, "%.17g", *angle);
    controlledGate(call, target, marker.mnemonic + "(" + text + ") " + qubit);
    return;
  }
  case MarkerKind::Measure:
  case MarkerKind::MeasureBytes: { // of a whole qubit variable (Taint.cpp checks it is whole)
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

// `%d = measure iN [ADDR]`: the bits of bytes that hold qubits are measured, the others read, and measured
// bytes are ordinary memory again (docs/DESIGN.md, "Qubits in memory").
void ControlWriter::measureBytes(const CallBase &call) {
  const auto *count = dyn_cast<ConstantInt>(call.getArgOperand(1));
  if (!count || count->getZExtValue() < 1 || count->getZExtValue() > 8) {
    notHandled("bennett_measure_bytes with a number of bytes that is not a constant from 1 to 8", &call);
  }
  unsigned bits = unsigned(8 * count->getZExtValue());
  std::string measured = bits == 64 ? slot(&call) : fresh("measured");
  line(measured + " = measure " + intType(bits) + " [" + operand(call.getArgOperand(0)) + "]");
  if (bits < 64) {
    line(slot(&call) + " = zext " + intType(bits) + " " + measured + " to i64");
  }
}

void ControlWriter::store(const StoreInst &store) {
  if (const MDNode *group = store.getMetadata(overwriteMetadata)) {
    exchangedStores(store, *group);
    return;
  }
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
      std::string callee = slice(computation, value, &target, target.bits, store, args, true);
      apply(callee, args, target, store);
      return;
    }
  }
  if (!known && fact.kind == Fact::Computed && recomputable(fact.value, target, store)) {
    // The old value is a function of registers unchanged since it was stored: applied into the variable again it
    // clears it (x ^= old), and the new value, with any read of the old one replaced by that computation, is
    // XORed in. Nothing is kept.
    bool coherent =
        !isa<Constant>(value) && (analysis.tainted(value) || (module.options.taintAll && sliceable(value)));
    std::optional<Slice> next;
    if (coherent) {
      next = collect(value, store, true);
    }
    bool readsItself = next && std::any_of(next->loads.begin(), next->loads.end(), [&](const LoadInst *load) {
      return analysis.access(load->getPointerOperand())->storage == &target;
    });
    if (!readsItself) {
      std::string args;
      std::string callee = slice(collect(fact.value, store, true), fact.value, &target, target.bits, store, args);
      apply(callee, args, target, store);
      if (!next) {
        xorClassical(target, value, store);
        return;
      }
      std::string nextArgs;
      std::string nextCallee = slice(*next, value, &target, target.bits, store, nextArgs);
      apply(nextCallee, nextArgs, target, store);
      return;
    }
  }
  if (!known) {
    if (target.alloca->getMetadata(sharedMetadata)) {
      // The front placed this clearing where computing the value again gives the value held (Evaluations.h);
      // keeping the old value instead would release a register that is not 0.
      notHandled("clearing the register of a value with several consumers, whose value cannot be computed again "
                 "here from what it read (route: a place to clear it that the allow-list in Evaluations.h accepts)",
                 &store);
    }
    retainedOverwrite(target, value, store);
    return;
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

// The exchange form of a store (docs/DESIGN.md, "Overwrites that keep the old value"): with n a fresh
// register, `apply g(a, x) -> n` (x, the variable, is an argument, n the target), three CNOTs per bit exchange
// n and x, so x holds g(a, x_old) and n holds x_old, which nothing kept can compute back here; n is released
// at once. Where the old value was the same for every configuration present nothing is lost; where it
// differed, interference between those configurations is gone, which is physics: the old values are what
// distinguishes them. One register of the variable's width is kept per executed overwrite.
void ControlWriter::retainedOverwrite(const Storage &target, const Value *value, const StoreInst &store) {
  const std::string &reg = registers.at(&target);
  unsigned bits = target.bits;
  std::string spare = fresh(target.alloca->getName().str() + ".old");
  line(spare + " = qalloc " + std::to_string(bits));
  if (const auto *constant = dyn_cast<ConstantInt>(resolve(value))) {
    for (unsigned bit = 0; bit < bits; ++bit) {
      if (constant->getValue().getZExtValue() >> bit & 1) {
        line("x " + spare + "[" + std::to_string(bit) + "]");
      }
    }
  } else if (!analysis.tainted(value) && !(module.options.taintAll && sliceable(value))) {
    apply(module.copyFunction(bits), converted(value, bits), spare, store);
  } else {
    std::string args;
    std::string callee = slice(collect(value, store), value, nullptr, bits, store, args);
    apply(callee, args, spare, store);
  }
  for (unsigned bit = 0; bit < bits; ++bit) {
    std::string x = reg + "[" + std::to_string(bit) + "]";
    std::string n = spare + "[" + std::to_string(bit) + "]";
    line("x " + n + " ctrl " + x);
    line("x " + x + " ctrl " + n);
    line("x " + n + " ctrl " + x);
  }
  line("qfree " + spare);
  warning("the overwrite of qubit variable " + target.name +
              ", whose old value is not known here and cannot be computed back from what is kept, keeps the old "
              "value in " +
              std::to_string(bits) +
              " qubits each time it runs, released at once (the partial trace): interference between "
              "configurations with different old values is lost (coherence)",
          &store);
}

// The stores by exchange of the variables a region read and overwrote in a loop (docs/DESIGN.md, "Code under a
// qubit"): for each variable x of the group, with n a fresh register, `apply g(a, x_old) -> n` for every
// variable before any exchange (each g reads the old values of all of them), then three CNOTs per bit exchange
// n and x, so that x holds the final value and n the old one, and then n is cleared: by the old value where it
// was known, or by computing it again where it is a value computed from registers unchanged since and not among
// the group's (`recomputable`). Then n is 0 and released. Where the old value is neither, n keeps it and is
// released at once (the retained form of "Overwrites that keep the old value"), with a warning.
void ControlWriter::exchangedStores(const StoreInst &first, const MDNode &group) {
  if (exchangedDone.count(&first)) {
    return;
  }
  std::vector<const StoreInst *> stores;
  for (const Instruction &inst : *first.getParent()) {
    const auto *store = dyn_cast<StoreInst>(&inst);
    if (store && store->getMetadata(overwriteMetadata) == &group) {
      stores.push_back(store);
      exchangedDone.insert(store);
    }
  }
  std::set<const Storage *> members;
  for (const StoreInst *store : stores) {
    members.insert(analysis.access(store->getPointerOperand())->storage);
  }
  std::vector<std::string> spares;
  for (const StoreInst *store : stores) {
    const Storage &target = *analysis.access(store->getPointerOperand())->storage;
    std::string spare = fresh(target.alloca->getName().str() + ".old");
    spares.push_back(spare);
    line(spare + " = qalloc " + std::to_string(target.bits));
    std::string args;
    const Value *value = store->getValueOperand();
    std::string callee = slice(collect(value, first), value, nullptr, target.bits, first, args);
    apply(callee, args, spare, first);
  }
  for (size_t i = 0; i < stores.size(); ++i) {
    const Storage &target = *analysis.access(stores[i]->getPointerOperand())->storage;
    for (unsigned bit = 0; bit < target.bits; ++bit) {
      std::string x = registers.at(&target) + "[" + std::to_string(bit) + "]";
      std::string n = spares[i] + "[" + std::to_string(bit) + "]";
      line("x " + n + " ctrl " + x);
      line("x " + x + " ctrl " + n);
      line("x " + n + " ctrl " + x);
    }
  }
  for (size_t i = 0; i < stores.size(); ++i) {
    const Storage &target = *analysis.access(stores[i]->getPointerOperand())->storage;
    Fact old = analysis.before(&first, &target);
    if (old.kind == Fact::Known && analysis.dominators.dominates(old.value, &first)) {
      if (const auto *constant = dyn_cast<ConstantInt>(old.value)) {
        for (unsigned bit = 0; bit < target.bits; ++bit) {
          if (constant->getValue().getZExtValue() >> bit & 1) {
            line("x " + spares[i] + "[" + std::to_string(bit) + "]");
          }
        }
      } else {
        apply(module.copyFunction(target.bits), converted(old.value, target.bits), spares[i], first);
      }
    } else {
      std::optional<Slice> clearing;
      if (old.kind == Fact::Computed && recomputable(old.value, target, first)) {
        clearing = collect(old.value, first, true);
      }
      if (!clearing || std::any_of(clearing->quantum.begin(), clearing->quantum.end(),
                                   [&](const Storage *storage) { return members.count(storage) != 0; })) {
        // The retained form: n keeps the old value and is released as it is (the partial trace).
        line("qfree " + spares[i]);
        warning("the overwrite of qubit variable " + target.name +
                    " in a loop under the control of a value that depends on a qubit, whose old value is neither "
                    "known here nor computable again from what is kept, keeps the old value in " +
                    std::to_string(target.bits) +
                    " qubits each time it runs, released at once (the partial trace): interference between "
                    "configurations with different old values is lost (coherence); if the applied function "
                    "faults for some argument values, interference between those values and the rest can be "
                    "lost when the fault register is released",
                stores[i]);
        continue;
      }
      std::string args;
      std::string callee = slice(*clearing, old.value, nullptr, target.bits, first, args);
      apply(callee, args, spares[i], first);
    }
    line("qfree " + spares[i]);
  }
}

bool ControlWriter::recomputable(const Value *old, const Storage &target, const Instruction &store,
                                 std::set<const Value *> &active) const {
  if (!analysis.dominators.dominates(old, &store) || !active.insert(old).second) {
    return false; // or a cycle: `old` is being checked further up
  }
  struct Leave {
    std::set<const Value *> &active;
    const Value *old;
    ~Leave() { active.erase(old); }
  } leave{active, old};
  std::set<const Value *> seen;
  std::vector<const Value *> work{old};
  std::vector<const CallBase *> calls;
  while (!work.empty()) {
    const Value *value = resolve(work.back());
    work.pop_back();
    if (!seen.insert(value).second || isa<Constant>(value)) {
      continue;
    }
    if (analysis.isQuantumLoad(value)) {
      const auto *load = cast<LoadInst>(value);
      std::optional<Access> source = analysis.access(load->getPointerOperand());
      if (knownAt(*load, store)) {
        continue; // it read a known ordinary value, passed in
      }
      if (!source || source->storage == &target || !analysis.unchangedBetween(load, &store, source->storage)) {
        // It read a value computed from registers unchanged since, which collect (with knownLoads) computes
        // again in its place, as here.
        Fact fact = source && source->offset == 0 ? analysis.before(load, source->storage) : Fact{};
        if (fact.kind == Fact::Computed && widthOf(load->getType(), load) == source->storage->bits &&
            recomputable(fact.value, *source->storage, store, active)) {
          continue;
        }
        return false;
      }
      continue;
    }
    if (!analysis.tainted(value) && !(module.options.taintAll && sliceable(value))) {
      continue; // an ordinary value; it dominates `old`, so `store`
    }
    const auto *inst = cast<Instruction>(value);
    if (const auto *load = dyn_cast<LoadInst>(inst)) {
      const auto *global = dyn_cast<GlobalVariable>(getUnderlyingObject(load->getPointerOperand()));
      if (!global || !global->isConstant() || load->isVolatile()) {
        return false;
      }
      work.push_back(load->getPointerOperand());
    } else if (const auto *call = dyn_cast<CallBase>(inst)) {
      if (isa<IntrinsicInst>(call) || !module.isApplicable(call->getCalledFunction()) ||
          (sharesMemory(*call) && !analysis.memoryUnchangedBetween(call, &store))) {
        return false;
      }
      if (analysis.tainted(call)) {
        calls.push_back(call);
      }
      for (const Use &arg : call->args()) {
        work.push_back(arg.get());
      }
    } else if (sliceable(inst) || isa<GetElementPtrInst>(inst)) {
      for (const Use &op : inst->operands()) {
        work.push_back(op.get());
      }
    } else {
      return false; // a phi among them: merged where ordinary control flow joins
    }
  }
  return calls.size() < 2 || std::all_of(calls.begin(), calls.end(), [&](const CallBase *call) {
           return call->onlyReadsMemory() || !sharesMemory(*call);
         });
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
  Slice computation = collect(value, at);
  // A load whose variable changed before `at` reads, there, the value the variable held where it was loaded: a
  // known ordinary value, or one computed again from registers unchanged since (collect with knownLoads).
  if (std::any_of(computation.loads.begin(), computation.loads.end(), [&](const LoadInst *load) {
        return !analysis.unchangedBetween(load, &at, analysis.access(load->getPointerOperand())->storage);
      })) {
    computation = collect(value, at, true);
  }
  std::string callee = slice(computation, value, &target, target.bits, at, args);
  apply(callee, args, target, at);
}

std::string ControlWriter::converted(const Value *value, unsigned bits, bool typedConstant) {
  unsigned from = widthOf(value->getType(), value);
  if (isa<Constant>(resolve(value))) {
    // operand() prints the constant unsigned, in at most 128 bits.
    APInt n = APInt(128, operand(value), 10).trunc(bits);
    return (typedConstant ? intType(bits) + " " : "") + toString(n, 10, false);
  }
  if (from == bits) {
    return operand(value);
  }
  std::string result = fresh("converted");
  line(result + " = " + (from > bits ? "trunc " : "zext ") + intType(from) + " " + operand(value) + " to " +
       intType(bits));
  return result;
}

// Applied whole where its result is stored: its writes are undone when it ends, so a call whose result nothing
// uses would do nothing at all.
void ControlWriter::requireUsedResult(const CallBase &call) const {
  if (!call.onlyReadsMemory() && call.use_empty()) {
    notHandled("a call that depends on a qubit, whose result is not used, to a function that may write "
               "memory (applied, its writes would be undone; writes that must survive are not handled yet)",
               &call);
  }
}

void ControlWriter::requireApplicable(const CallBase &call) const {
  if (isa<IntrinsicInst>(call) || !module.isApplicable(call.getCalledFunction())) {
    notHandled("a call that depends on a qubit, to a function that performs quantum operations, is not defined, "
               "or is called through a pointer",
               &call);
  }
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

const Value *ControlWriter::knownAt(const LoadInst &load, const Instruction &sink) const {
  std::optional<Access> source = analysis.access(load.getPointerOperand());
  if (!source || source->offset != 0) {
    return nullptr;
  }
  Fact fact = analysis.before(&load, source->storage);
  if (fact.kind != Fact::Known || !analysis.dominators.dominates(fact.value, &sink)) {
    return nullptr;
  }
  // An instruction on a cycle may compute another value between the load and `sink`.
  const auto *inst = dyn_cast<Instruction>(fact.value);
  return inst && analysis.loops.getLoopFor(inst->getParent()) ? nullptr : fact.value;
}

Slice ControlWriter::collect(const Value *root, const Instruction &sink, bool knownLoads) {
  Slice slice;
  std::set<const Value *> seen;
  std::vector<const CallBase *> calls;
  std::function<void(const Value *)> visit = [&](const Value *value) {
    value = resolve(value);
    if (!seen.insert(value).second || isa<Constant>(value)) {
      return;
    }
    if (analysis.isQuantumLoad(value)) {
      const auto *load = cast<LoadInst>(value);
      if (const Value *known = knownLoads ? knownAt(*load, sink) : nullptr) {
        slice.known.push_back({load, known});
        return;
      }
      if (knownLoads) {
        std::optional<Access> source = analysis.access(load->getPointerOperand());
        Fact fact = source && source->offset == 0 ? analysis.before(load, source->storage) : Fact{};
        // A register for a value with several consumers is read, never computed again (Evaluations.h).
        if (source && source->storage->alloca->getMetadata(sharedMetadata) &&
            analysis.unchangedBetween(load, &sink, source->storage)) {
          fact = Fact{};
        }
        if (fact.kind == Fact::Computed && widthOf(load->getType(), load) == source->storage->bits &&
            recomputable(fact.value, *source->storage, sink)) {
          slice.computed.push_back({load, fact.value});
          visit(fact.value);
          return;
        }
      }
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
      requireApplicable(*call);
      // The applied function runs where the value is stored, not where the call was: it must start from the
      // same memory there.
      if (sharesMemory(*call) && !analysis.memoryUnchangedBetween(call, &sink)) {
        notHandled("a call that depends on a qubit and reads memory that may change before its result is used",
                   inst);
      }
      // Each applied call starts from the memory as it was and its writes are undone when it ends; in one
      // computation a second call would see the first's writes. Only calls that depend on a qubit count: an
      // ordinary call `--taint-all` moves into the slice reads memory at most and runs first (below).
      if (analysis.tainted(call)) {
        calls.push_back(call);
      }
      if (calls.size() > 1 && std::any_of(calls.begin(), calls.end(), [&](const CallBase *c) {
            return !c->onlyReadsMemory() && sharesMemory(*c);
          })) {
        notHandled("two calls that depend on a qubit in one computation, one of which may write memory (each would "
                   "have to start from the memory as it was)",
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
  // What does not depend on a qubit (moved into the slice by `--taint-all`) never depends on what does, so it
  // can run first: an ordinary call then reads memory as it is at its own site, before any applied call in
  // the slice writes the private copy.
  std::stable_partition(slice.body.begin(), slice.body.end(),
                        [&](const Instruction *inst) { return !analysis.tainted(inst); });
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
  return holds || provesFlipByCases(slice, root, target);
}

// The simplifier does not always fold the identity: `y ^= q && !r` on a `_Bool` is `y != (q ? !r : 0)`, and
// InstCombine rewrites the negated `select` by De Morgan into another `select`, so `(s ^ 1) ^ s` no longer
// meets itself. When the stored value reads only qubit variables (no ordinary value, which could be
// anything), every assignment of their support bits is a case: the identity holds if each case folds to
// v(1) = v(0) ^ 1. The support is sound once the loads carry their ranges (annotateRanges), so these are
// all the values the variables can hold.
bool ControlWriter::provesFlipByCases(const Slice &slice, const Value *root, const Storage &target) {
  if (!slice.classical.empty()) {
    return false;
  }
  std::vector<std::pair<const Storage *, unsigned>> bits; // the other variables' support bits
  for (const Storage *storage : slice.quantum) {
    if (storage == &target) {
      continue;
    }
    for (unsigned bit = 0; bit < storage->bits; ++bit) {
      if (storage->support >> bit & 1) {
        bits.push_back({storage, bit});
      }
    }
  }
  if (bits.size() > maxEnumeratedSupportBits) {
    return false;
  }
  for (uint64_t assignment = 0; assignment < uint64_t(1) << bits.size(); ++assignment) {
    std::map<const Storage *, uint64_t> values;
    for (const Storage *storage : slice.quantum) {
      values[storage] = 0;
    }
    for (size_t i = 0; i < bits.size(); ++i) {
      if (assignment >> i & 1) {
        values[bits[i].first] |= uint64_t(1) << bits[i].second;
      }
    }
    std::optional<uint64_t> folded[2];
    for (uint64_t bit = 0; bit < 2; ++bit) {
      values[&target] = bit;
      folded[bit] = evaluate(slice, root, values);
    }
    if (!folded[0] || !folded[1] || (*folded[0] ^ *folded[1]) != 1) {
      return false;
    }
  }
  return true;
}

void ControlWriter::requireCurrent(const Slice &slice, const Storage *target, const Instruction &sink) {
  for (const LoadInst *load : slice.loads) {
    const Storage *storage = analysis.access(load->getPointerOperand())->storage;
    if (storage != target && !analysis.unchangedBetween(load, &sink, storage)) {
      notHandled("the value of qubit variable " + storage->name + " read here is used after the variable changed (at " +
                     locationOf(&sink) + ")",
                 load);
    }
  }
}

// The function computing `root` from its leaves: ordinary values (passed in) and qubit variables (whose
// registers are passed in and read as integers). Every instruction of the computation dominates the sink,
// so the function computes exactly what the control program would have computed there. Loads of the
// target itself read 0: they occur only where provesFlip has shown that this gives the XOR form.
std::string ControlWriter::slice(const Slice &slice, const Value *root, const Storage *target, unsigned bits,
                                 const Instruction &sink, std::string &args, bool flipProved) {
  requireCurrent(slice, target, sink);
  bool inPlace = false;
  for (const LoadInst *load : slice.loads) {
    inPlace = inPlace || analysis.access(load->getPointerOperand())->storage == target;
  }
  if (inPlace && !flipProved) {
    notHandled("an update of qubit variable " + target->name + " computed from its own value; only `" +
                   plainName(*target) + " ^= expression` without it, and in-place bijections, are handled",
               &sink);
  }

  BodyWriter writer(module, &fn);
  writer.shareAliases(*this);
  std::string params;
  for (const Value *value : slice.classical) {
    std::string name = writer.fresh(value->hasName() ? value->getName().str() : "value");
    writer.bind(value, name);
    params += (params.empty() ? "" : ", ") + intType(widthOf(value->getType(), value)) + " " + name;
    args += (args.empty() ? "" : ", ") + typed(value); // an argument of `apply` that is a constant has its type
  }
  for (auto [load, value] : slice.computed) {
    writer.alias(load, value);
  }
  // A known value that is a constant is written in place (as the control program's own aliases do), so equal
  // computations have equal text whether a load was known here or elsewhere.
  std::set<const Value *> knownZero;
  for (auto [load, value] : slice.known) {
    if (const auto *constant = dyn_cast<ConstantInt>(value); constant && value->getType() == load->getType()) {
      writer.alias(load, constant);
      if (constant->isZero()) {
        knownZero.insert(load);
      }
      continue;
    }
    std::string name = writer.fresh("known");
    unsigned width = widthOf(load->getType(), load);
    writer.bind(load, name);
    params += (params.empty() ? "" : ", ") + intType(width) + " " + name;
    args += (args.empty() ? "" : ", ") + converted(value, width, true);
  }
  std::map<const Storage *, std::string> qubits;
  for (const Storage *storage : slice.quantum) {
    if (storage == target) {
      continue;
    }
    std::string name = writer.fresh(storage->alloca->hasName() ? storage->alloca->getName().str() : "qubits");
    qubits[storage] = name;
    params += (params.empty() ? "" : ", ") + intType(storage->bits) + " " + name;
    args += (args.empty() ? "" : ", ") + registers.at(storage);
  }
  for (const LoadInst *load : slice.loads) {
    Access source = *analysis.access(load->getPointerOperand());
    if (source.storage == target) {
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
  // `v ^ 0`, `v | 0` and `v + 0` are `v`: the start of a register the front knows to be 0 (a load aliased to that
  // constant) leaves them in the C function. Written as `v`, the computation that sets a register and the one
  // that clears it again are the same text, so the same generated function (Evaluations.h).
  for (const Instruction *inst : slice.body) {
    const auto *binary = dyn_cast<BinaryOperator>(inst);
    if (!binary || (binary->getOpcode() != Instruction::Xor && binary->getOpcode() != Instruction::Or &&
                    binary->getOpcode() != Instruction::Add)) {
      continue;
    }
    for (unsigned side : {0u, 1u}) {
      const Value *operand = binary->getOperand(side);
      if (isZero(resolve(operand)) || knownZero.count(resolve(operand))) {
        writer.alias(inst, binary->getOperand(1 - side));
        break;
      }
    }
  }
  for (const Instruction *inst : slice.body) {
    writer.instruction(*inst);
  }
  if (widthOf(root->getType(), root) != bits) {
    fail("internal: a stored value of the wrong width");
  }
  writer.line("ret " + writer.typed(root));
  std::string text = "(" + params + ") -> " + intType(bits) + " {\nentry:\n" + writer.out + "}\n";
  std::set<const MDNode *> evaluations = sliceEvaluations(slice);
  std::string name = module.addFunction(fn.getName().str() + ".apply", text);
  evaluated[name].insert(evaluations.begin(), evaluations.end());
  return name;
}

std::set<const MDNode *> ControlWriter::sliceEvaluations(const Slice &slice) {
  std::set<const MDNode *> evaluations;
  for (const Instruction *inst : slice.body) {
    // Every evaluation the body copies, an ordinary call `--taint-all` moves in included.
    if (const MDNode *evaluation = evaluationOf(*inst)) {
      if (analysis.tainted(inst)) {
        qubitEvaluations.insert(evaluation);
      }
      evaluations.insert(evaluation);
      noteEvaluation(evaluation, *inst);
    }
    const auto *call = dyn_cast<CallBase>(inst);
    if (!call) {
      continue;
    }
    if (module.evaluations().readsCallerFrames(*call)) {
      notHandled("an applied call that may reach `__builtin_return_address` above level 0: it would run under a "
                 "frame the C program does not have, and an ancestor's return address would be that frame's "
                 "(route: count call depth, or let generated frames not count, a change to the device program)",
                 inst);
    }
    const Function *callee = call->getCalledFunction();
    if (!callee || callee->isDeclaration()) {
      continue;
    }
    for (const MDNode *evaluation : module.evaluations().inside(callee)) {
      if (readsPosition(evaluation)) {
        notHandled(describeEvaluation(evaluation) +
                       " of the function under the control of a value that depends on a qubit: in the applied "
                       "function it would be that function's own (route: compute it before that code and pass it "
                       "in, not built)",
                   inst);
      }
      evaluations.insert(evaluation);
      qubitEvaluations.insert(evaluation);
      noteEvaluation(evaluation, *inst);
    }
  }
  return evaluations;
}

void ControlWriter::requireSingleEvaluations() const {
  std::map<const MDNode *, std::set<std::string>> computers;
  for (const auto &[name, evaluations] : evaluated) {
    for (const MDNode *evaluation : evaluations) {
      if (qubitEvaluations.count(evaluation)) {
        computers[evaluation].insert(name);
      }
    }
  }
  for (auto &[evaluation, names] : computers) {
    bool ordinary = ordinaryEvaluations.count(evaluation) != 0;
    if (names.size() + (ordinary ? 1 : 0) > 1) {
      auto why = withoutRegister.find(evaluation);
      notHandled(describeEvaluation(evaluation) + " whose value is consumed in " +
                     std::to_string(names.size() + (ordinary ? 1 : 0)) + " places, each computing it again " +
                     (ordinary ? "(one of them ordinary code)" : "in a function of its own") +
                     ": the copies run in different frames and may give different values; " +
                     (why != withoutRegister.end()
                          ? "the register of its own that the front gives such a value is cleared by computing the "
                            "value again after its consumers, and here " +
                                why->second
                      : registered.count(evaluation)
                          ? "the register of its own that the front gave it is cleared before a measurement or a "
                            "change of what it reads, and a later use (the overwrite of a variable that held a value "
                            "computed from it) computes it again after that (route: keep what it reads until that "
                            "use, not built)"
                          : "the front gives such a value a register of its own only where the control program "
                            "computes it, outside code under a qubit, and when it is at most 64 bits wide (route: "
                            "the register inside predicated code, and wider registers, not built)"),
                 evaluationSite.at(evaluation));
    }
  }
  for (const auto &[name, evaluations] : evaluated) {
    auto sites = appliedAt.find(name);
    if (!evaluations.empty() && sites != appliedAt.end() && sites->second.size() > 1) {
      requireSameStack(sites->second, **evaluations.begin());
    }
  }
}

// Whether `to` can run after `from` on a path that does not return to `from` first.
bool reachesBefore(const Instruction &from, const Instruction &to) {
  if (from.getParent() == to.getParent() && from.comesBefore(&to)) {
    return true;
  }
  std::set<const BasicBlock *> seen;
  std::vector<const BasicBlock *> work(succ_begin(from.getParent()), succ_end(from.getParent()));
  while (!work.empty()) {
    const BasicBlock *block = work.back();
    work.pop_back();
    if (!seen.insert(block).second) {
      continue;
    }
    if (block == to.getParent() && (block != from.getParent() || to.comesBefore(&from))) {
      return true;
    }
    if (block != from.getParent()) {
      work.insert(work.end(), succ_begin(block), succ_end(block));
    }
  }
  return false;
}

void ControlWriter::requireSameStack(const std::vector<const Instruction *> &sites,
                                     const MDNode &evaluation) const {
  for (const Instruction &inst : instructions(fn)) {
    const auto *local = dyn_cast<AllocaInst>(&inst);
    const auto *intrinsic = dyn_cast<IntrinsicInst>(&inst);
    bool moves = (local && !analysis.basedOn(local) && !local->getMetadata(qubitMetadata)) ||
                 (intrinsic && intrinsic->getIntrinsicID() == Intrinsic::stackrestore);
    if (!moves) {
      continue;
    }
    for (const Instruction *first : sites) {
      for (const Instruction *second : sites) {
        if (first != second && reachesBefore(*first, inst) && reachesBefore(inst, *second)) {
          notHandled(describeEvaluation(&evaluation) +
                         " computed by one applied function at two places with a reservation or release of the "
                         "stack between them: the second run would start from another stack pointer (route: "
                         "clear the register earlier, before the reservation or release, or keep the stack pointer; not built)",
                     &inst);
        }
      }
    }
  }
}

// A gate, controlled by the condition in the call's bundle if it has one. A condition that is a conjunction
// of qubits becomes the gate's controls; any other is computed into a fresh qubit, controls the gate, and
// is computed again to return the qubit to 0 before it is released. Either way the condition must not read
// the gate's own variable, or the gate would change what controls it, and every qubit variable it reads must
// be unchanged between the read and the gate, since the gate reads the registers as they are there.
void ControlWriter::controlledGate(const CallBase &call, const Access &target, const std::string &gate) {
  if (std::optional<OperandBundleUse> qubits = call.getOperandBundle(qubitControlBundle)) {
    std::string controls;
    for (const Use &qubit : qubits->Inputs) {
      Access control = *analysis.access(qubit.get());
      if (control.storage == target.storage) {
        fail("internal: a gate controlled by a qubit of its own variable");
      }
      controls += (controls.empty() ? " ctrl " : ", ") + registers.at(control.storage) + "[" +
                  std::to_string(control.bit) + "]";
    }
    line(gate + controls);
    return;
  }
  std::optional<OperandBundleUse> bundle = call.getOperandBundle(controlBundle);
  if (!bundle) {
    line(gate);
    return;
  }
  const Value *condition = bundle->Inputs[0];
  if (const auto *constant = dyn_cast<ConstantInt>(resolve(condition))) {
    if (constant->isOne()) {
      line(gate);
    }
    return;
  }
  bool coherent = analysis.tainted(condition) || (module.options.taintAll && sliceable(condition));
  Slice computation;
  if (coherent) {
    computation = collect(condition, call);
    for (const Storage *storage : computation.quantum) {
      if (storage == target.storage) {
        notHandled("a gate on qubit variable " + storage->name + " under a condition that depends on " +
                       storage->name + " (it would change the condition it is controlled by)",
                   &call);
      }
    }
    // Literal controls name the registers as they are at the gate, as the computed condition reads them.
    requireCurrent(computation, nullptr, call);
    if (std::optional<std::vector<std::pair<std::string, bool>>> qubits = literals(computation, condition)) {
      std::string controls;
      for (const auto &[qubit, one] : *qubits) {
        controls += (controls.empty() ? " ctrl " : ", ") + qubit;
        if (!one) {
          line("x " + qubit);
        }
      }
      line(gate + controls);
      for (const auto &[qubit, one] : *qubits) {
        if (!one) {
          line("x " + qubit);
        }
      }
      return;
    }
  }
  std::string qubit = fresh("condition");
  line(qubit + " = qalloc 1");
  auto compute = [&] {
    if (!coherent) {
      apply(module.copyFunction(1), converted(condition, 1), qubit, call);
      return;
    }
    std::string args;
    std::string callee = slice(computation, condition, nullptr, 1, call, args);
    apply(callee, args, qubit, call);
  };
  compute();
  line(gate + " ctrl " + qubit + "[0]");
  compute();
  line("qfree " + qubit);
}

std::optional<std::vector<std::pair<std::string, bool>>> ControlWriter::literals(const Slice &slice,
                                                                                 const Value *condition) {
  // Every variable read must be one qubit as far as LLVM's known bits show (its support is one bit).
  std::vector<std::pair<const Storage *, unsigned>> qubits;
  for (const Storage *storage : slice.quantum) {
    if (storage->support == 0 || (storage->support & (storage->support - 1)) != 0) {
      return std::nullopt;
    }
    qubits.push_back({storage, unsigned(countr_zero(storage->support))});
  }
  // The condition is the conjunction of literals l_i exactly when it is 0 wherever one l_i is false,
  // whatever the others are, and 1 where all are true.
  std::vector<std::pair<std::string, bool>> result;
  std::map<const Storage *, uint64_t> all;
  for (const auto &[storage, bit] : qubits) {
    std::optional<bool> one;
    for (bool value : {true, false}) {
      std::optional<uint64_t> folded = evaluate(slice, condition, {{storage, value ? 0 : uint64_t(1) << bit}});
      if (folded && *folded == 0) {
        one = value;
        break;
      }
    }
    if (!one) {
      return std::nullopt;
    }
    all[storage] = *one ? uint64_t(1) << bit : 0;
    result.push_back({registers.at(storage) + "[" + std::to_string(bit) + "]", *one});
  }
  std::optional<uint64_t> folded = evaluate(slice, condition, all);
  if (!folded || *folded != 1) {
    return std::nullopt;
  }
  return result;
}

std::optional<uint64_t> ControlWriter::evaluate(const Slice &slice, const Value *root,
                                                const std::map<const Storage *, uint64_t> &assigned) {
  Module &module = *fn.getParent();
  LLVMContext &context = module.getContext();
  std::vector<Type *> params;
  for (const Value *value : slice.classical) {
    params.push_back(value->getType());
  }
  for (size_t i = 0; i < slice.quantum.size(); ++i) {
    params.push_back(PointerType::get(context, 0));
  }
  Function *scratch = Function::Create(FunctionType::get(root->getType(), params, false),
                                       GlobalValue::InternalLinkage, "bennett.evaluate", module);
  BasicBlock *entry = BasicBlock::Create(context, "entry", scratch);
  IRBuilder<> b(entry);
  ValueToValueMapTy map;
  for (size_t i = 0; i < slice.classical.size(); ++i) {
    map[slice.classical[i]] = scratch->getArg(unsigned(i));
  }
  for (const LoadInst *load : slice.loads) {
    Access source = *analysis.access(load->getPointerOperand());
    auto found = assigned.find(source.storage);
    if (found != assigned.end()) {
      map[load] = ConstantInt::get(load->getType(), found->second >> (8 * source.offset));
      continue;
    }
    size_t index = std::find(slice.quantum.begin(), slice.quantum.end(), source.storage) - slice.quantum.begin();
    Value *pointer = b.CreateConstInBoundsGEP1_64(b.getInt8Ty(), scratch->getArg(unsigned(slice.classical.size() + index)),
                                                  source.offset);
    LoadInst *copy = b.CreateLoad(load->getType(), pointer);
    copy->setMetadata(LLVMContext::MD_range, load->getMetadata(LLVMContext::MD_range));
    map[load] = copy;
  }
  std::vector<CallBase *> calls;
  for (const Instruction *inst : slice.body) {
    Instruction *copy = inst->clone();
    copy->setDebugLoc(DebugLoc());
    b.Insert(copy);
    RemapInstruction(copy, map, RF_NoModuleLevelChanges | RF_IgnoreMissingLocals);
    map[inst] = copy;
    if (auto *call = dyn_cast<CallBase>(copy); call && call->getCalledFunction() &&
                                               !call->getCalledFunction()->isDeclaration()) {
      calls.push_back(call);
    }
  }
  b.CreateRet(map.count(root) ? static_cast<Value *>(map[root]) : const_cast<Value *>(root));
  for (CallBase *call : calls) {
    InlineFunctionInfo info;
    (void)InlineFunction(*call, info); // a call that cannot be inlined stays, and nothing folds
  }
  simplifyDeep(*scratch);
  std::optional<uint64_t> result;
  if (scratch->size() == 1) {
    const auto *ret = cast<ReturnInst>(scratch->front().getTerminator());
    if (const auto *constant = dyn_cast<ConstantInt>(ret->getReturnValue())) {
      result = constant->getZExtValue();
    }
  }
  scratch->eraseFromParent();
  return result;
}

void ControlWriter::release(const Instruction &ret) {
  for (auto it = analysis.storages.rbegin(); it != analysis.storages.rend(); ++it) {
    const Storage &storage = **it;
    Fact fact = analysis.before(&ret, &storage);
    // A variable whose value is known is returned to zero first; any other is released as it is: the partial
    // trace, which the simulator reports when the variable is not zero (a report, never a fault).
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
