// Taint: which storage is a qubit variable, what is known about each one at each point, and which values
// depend on a qubit.
//
// A qubit variable is a local variable whose address a marker receives; it becomes one quantum register
// of the variable's width. At each point of the function a qubit variable is either Known (in a basis
// state whose value is an ordinary value available there: after its initial store, a classical store, or
// a measurement) or Quantum. A load of a Quantum variable is tainted, and so is every value computed from a
// tainted value. When unsure the analysis says Quantum and tainted, never less.
#pragma once

#include <llvm/Analysis/LoopInfo.h>
#include <llvm/IR/Dominators.h>

#include <map>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <vector>

namespace llvm {
class AllocaInst;
class Function;
class Instruction;
class LoadInst;
class Value;
} // namespace llvm

namespace bennett {

struct Storage {
  llvm::AllocaInst *alloca;
  unsigned bits;    // the register's width: the variable's size in bits, at most 64
  std::string name; // the C variable, for messages
  uint64_t support = ~uint64_t(0); // the bits that may be 1 (computeSupport)
  std::vector<llvm::Instruction *> mutations; // the stores into it and the markers applied to it
};

// A pointer into a qubit variable: the variable and a constant byte offset.
struct Access {
  Storage *storage;
  unsigned offset;
};

struct Fact {
  enum Kind { Unvisited, Known, Quantum };
  Kind kind = Unvisited;
  llvm::Value *value = nullptr; // when Known: the variable's value in the low `bits` bits
};

class ControlAnalysis {
public:
  // Analyses `function`. With `taintAll`, no load of a qubit variable is ever treated as classical.
  ControlAnalysis(llvm::Function &function, bool taintAll);

  std::vector<std::unique_ptr<Storage>> storages;
  llvm::DominatorTree dominators;
  llvm::LoopInfo loops;

  // The qubit variable `pointer` points into, if any.
  std::optional<Access> access(const llvm::Value *pointer) const;
  // What is known about `storage` just before `at` executes.
  Fact before(const llvm::Instruction *at, const Storage *storage) const;
  // Whether `value` is a load of a qubit variable that reads its quantum value.
  bool isQuantumLoad(const llvm::Value *value) const;
  bool tainted(const llvm::Value *value) const { return taint.count(value) != 0; }
  // A branch, switch or phi that depends on a qubit, if there is one: the first in reverse post-order.
  llvm::Instruction *seed() const;
  // Whether the value of `storage` read at `load` is still its value when `use` executes, on every path.
  bool unchangedBetween(const llvm::Instruction *load, const llvm::Instruction *use, const Storage *storage) const;
  // Whether no ordinary memory may be written between `from` and `use` (qubit variables are not memory).
  bool memoryUnchangedBetween(const llvm::Instruction *from, const llvm::Instruction *use) const;
  // Sets each variable's support: the bits gates act on, and the bits LLVM's known-bits analysis allows in
  // any value stored into it. Sound once the loads carry the ranges that this support implies (a fixed
  // point); `gatesOnly` gives the optimistic start of that iteration.
  void computeSupport(bool gatesOnly = false);

private:
  llvm::Function &function;
  bool taintAll;
  std::vector<llvm::BasicBlock *> order; // reverse post-order
  std::map<const llvm::BasicBlock *, std::vector<Fact>> entryFacts;
  std::set<const llvm::Value *> taint;
  std::set<const llvm::Value *> quantumLoads;

  void findStorage();
  void checkUses(Storage &storage);
  void runDataflow();
  void transfer(const llvm::Instruction &inst, std::vector<Fact> &facts) const;
  bool computeTaint();
  size_t indexOf(const Storage *storage) const;
};

} // namespace bennett
