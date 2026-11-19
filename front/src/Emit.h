// Writing the device program's text form. `bennett-front` only writes text; the one reader and validator
// of the device program are in crates/ir.
#pragma once

#include "Aliasing.h"
#include "Applied.h"
#include "Evaluations.h"

#include <cstdint>
#include <map>
#include <memory>
#include <optional>
#include <set>
#include <string>
#include <vector>

namespace llvm {
class BasicBlock;
class Constant;
class DataLayout;
class Function;
class GlobalVariable;
class Instruction;
class Module;
class Type;
class Value;
} // namespace llvm

namespace bennett {

struct Options {
  uint64_t regionBudget = 0; // steps for each run of an applied function
  uint64_t stackBytes = 0;   // memory above the static data, for the stack
  bool taintAll = false;     // treat every value that can run coherently as if it depended on a qubit
};

// Unique names in one scope, made from LLVM names where they are usable.
class Namer {
public:
  std::string fresh(const std::string &hint);

private:
  std::set<std::string> used;
  std::map<std::string, unsigned> next; // the last suffix given for each base
};

class ModuleWriter {
public:
  ModuleWriter(llvm::Module &module, const Options &options, const std::set<const llvm::Function *> &control);

  // The whole program text.
  std::string write();

  const Options &options;
  const llvm::DataLayout &layout() const;
  bool isControl(const llvm::Function *function) const { return control.count(function) != 0; }
  // Whether calls of `function` may run coherently: defined, no quantum operations, and LLVM shows it
  // writes no memory that outlives the call.
  bool isCoherent(const llvm::Function *function) const;
  // Whether a call of `function` whose arguments depend on a qubit may be applied whole: defined and with no
  // quantum operations, whatever memory it writes (its writes are undone when the applied call ends).
  bool isApplicable(const llvm::Function *function) const;
  std::string functionName(const llvm::Function *function) const;
  // Refuses to take the address of `function` if LLVM may assume more alignment of it than its device
  // address (its index plus 1) has.
  void checkFunctionAddress(const llvm::Function *function) const;
  // The integer an LLVM constant denotes: integers, null, addresses of globals and functions, and constant
  // expressions over them. Undefined values denote 0.
  std::optional<uint64_t> constantValue(const llvm::Constant *constant) const;
  // Adds a generated function; `body` is its text after the name. Returns "@name". A body equal to one added
  // before gives that function: one computation in C is one generated function however often it is applied.
  std::string addFunction(const std::string &hint, const std::string &body);
  // `@bennett.copy.iN`: returns its argument, for XORing an ordinary value into a register with `apply`.
  std::string copyFunction(unsigned bits);
  // What the module's functions write and read, for the warning about an applied call's undone writes;
  // computed on first use.
  const AppliedCalls &appliedCalls();
  // Which memory the module's pointers and calls may reach (Aliasing.h); computed on first use.
  const Aliasing &aliasing();
  // The evaluations in the module's functions (Evaluations.h); computed on first use.
  Evaluations &evaluations();
  // Records a call the control program makes as ordinary code (for refuseAppliedCallsRunOrdinarily, Classify.h).
  void noteOrdinaryCall(const llvm::CallBase &call) { ordinaryCalls.push_back(&call); }

private:
  llvm::Module &module;
  const std::set<const llvm::Function *> &control;
  std::map<const llvm::Function *, std::string> names;
  std::map<const llvm::Function *, uint64_t> indices;
  std::map<const llvm::GlobalVariable *, uint64_t> addresses;
  std::map<unsigned, std::string> copies;
  std::unique_ptr<AppliedCalls> applied;
  std::unique_ptr<Aliasing> reach;
  std::unique_ptr<Evaluations> repeated;
  std::map<std::string, std::string> bodies; // a generated function's text after its name: its name
  std::vector<std::string> generated;
  std::vector<const llvm::CallBase *> ordinaryCalls;
  Namer functionNames;
  uint64_t staticEnd = 0;
  size_t definedCount = 0;

  void layoutGlobals();
  std::string dataSegments() const;
};

// Writes one function body. The control program's writer extends it.
class BodyWriter {
public:
  BodyWriter(ModuleWriter &module, const llvm::Function *function);
  virtual ~BodyWriter() = default;

  // The whole `func` text of an ordinary LLVM function.
  std::string function();

  // Naming. `operand` is a slot or an untyped constant; `typed` a slot or a typed constant.
  std::string slot(const llvm::Value *value);
  std::string operand(const llvm::Value *value);
  std::string typed(const llvm::Value *value);
  std::string fresh(const std::string &hint) { return "%" + names.fresh(hint); }
  void bind(const llvm::Value *value, const std::string &slotName) { slots[value] = slotName; }
  void alias(const llvm::Value *value, const llvm::Value *to) { aliases[value] = to; }
  // Takes over `other`'s aliases, for a function that computes part of `other`'s function.
  void shareAliases(const BodyWriter &other) { aliases.insert(other.aliases.begin(), other.aliases.end()); }
  static unsigned widthOf(const llvm::Type *type, const llvm::Value *at);
  static std::string intType(unsigned bits) { return "i" + std::to_string(bits); }

  void line(const std::string &text) { out += "    " + text + "\n"; }
  // Lowers one non-terminator; the result, if any, is named slot(&inst).
  virtual void instruction(const llvm::Instruction &inst);
  // Values that read as other values: casts that change nothing.
  void findAliases(const llvm::Function &function);

  std::string out;

protected:
  ModuleWriter &module;
  const llvm::Function *current;
  Namer names;
  std::map<const llvm::Value *, std::string> slots;
  std::map<const llvm::Value *, const llvm::Value *> aliases;
  std::map<const llvm::BasicBlock *, std::string> labels;

  const llvm::Value *resolve(const llvm::Value *value) const;
  std::string label(const llvm::BasicBlock *block);
  std::string target(const llvm::BasicBlock *from, const llvm::BasicBlock *to);
  std::string header(const llvm::Function &function);
  virtual void terminator(const llvm::Instruction &term);
  // Called after the entry block's label, before its first instruction.
  virtual void prologue() {}
  void body(const llvm::Function &function);
  void call(const llvm::Instruction &inst);
  void address(const llvm::Instruction &inst);
};

// Writes a function that performs quantum operations.
std::string writeControlFunction(ModuleWriter &module, llvm::Function &function);

} // namespace bennett
