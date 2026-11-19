// Evaluating twice, and running in a generated function (docs/DESIGN.md, "Evaluations the front repeats or moves").
//
// The front applies a computation where its value is consumed: once per consumer, and once more to clear a
// register. Each such `apply` runs a function the front generates, which calls what the C code called. A run of
// an applied function is determined by the function, its arguments, its budget, the frames below it with their
// positions, the stack pointer, and the bytes of memory it reads (and whether they hold qubits). So:
//
// - Arithmetic and reads of memory unchanged between the places (checked elsewhere) may be computed by as many
//   generated functions as there are consumers: they give the same value in each.
// - Anything else is one *evaluation*: a call, a reservation of stack, a saved stack pointer, a return address.
//   Its result may depend on the frames around it (the address of a local, `__builtin_return_address`). One
//   evaluation computed again to clear its register is one generated function (equal text, equal function),
//   applied with the same arguments from the same running call of the control function, with no reservation or
//   release of its stack between the applications. An evaluation whose value has several consumers gets a register
//   of its own (registerSharedEvaluations); where it cannot, computing it in two generated functions, or in one and
//   in ordinary code, is refused by name.
// - Code in a generated function sits under a frame the C program does not have. A return address or saved
//   stack pointer of the C function is never moved into one (code under a qubit containing one is refused), and
//   nothing applied may reach, through its calls, a read of a return address above level 0 (an ancestor's
//   return address would be the generated frame's). A call through a pointer may reach every function whose
//   address is taken.
//
// The evaluations are named on the control functions before outlining (`markEvaluations`): each instruction
// gets a distinct metadata node, which every copy the front makes keeps, so the copies of one evaluation can be
// counted when the device program is written.
#pragma once

#include <map>
#include <set>
#include <string>

namespace llvm {
class CallBase;
class Function;
class Instruction;
class MDNode;
class Module;
} // namespace llvm

namespace bennett {

// One register for an evaluation with several consumers (design note 5.2). For each call in `function` whose
// value depends on a qubit and reaches two or more consumers (stores, jumps, calls, returns) through arithmetic,
// the value where those uses part (the call itself or the last arithmetic all of them go through) is stored once
// into a fresh qubit variable where C computes it, every consumer reads the variable, and the variable is set to 0
// where all consumers are done: the front then computes the value into the register once, and clears it by the same
// computation (Control.cpp, an overwrite that computes the old value again). Values wider than 64 bits, and those
// without a point after all consumers that only the value's own block reaches, are left as they are.
void registerSharedEvaluations(llvm::Function &function, bool taintAll);

// The metadata that marks such a register: a computation that reads it reads it, and never computes its value
// again (that would be a second evaluation).
inline constexpr const char *sharedMetadata = "bennett.shared.register";

// Names each evaluation in the functions `control`.
void markEvaluations(llvm::Module &module, const std::set<const llvm::Function *> &control);

// The evaluation `inst` is (a copy of), if any.
const llvm::MDNode *evaluationOf(const llvm::Instruction &inst);

// What an evaluation is, for messages: "a call to @f", "a reservation of stack", ...
std::string describeEvaluation(const llvm::MDNode *evaluation);

// Whether the evaluation reads its frame's position: a return address or a saved stack pointer.
bool readsPosition(const llvm::MDNode *evaluation);

// Evaluations inside the module's functions, and the functions that may read a return address above level 0.
class Evaluations {
public:
  explicit Evaluations(const llvm::Module &module);

  // The evaluations `function` contains, and those of the functions it calls that contain any (the functions the
  // front generated from control functions), transitively.
  const std::set<const llvm::MDNode *> &inside(const llvm::Function *function);
  // Whether `call` may reach, through its callee and what that calls, `__builtin_return_address(N)` with N >= 1.
  bool readsCallerFrames(const llvm::CallBase &call) const;

private:
  std::map<const llvm::Function *, std::set<const llvm::MDNode *>> direct;
  std::map<const llvm::Function *, std::set<const llvm::MDNode *>> closed;
  std::set<const llvm::Function *> callerReaders;
  bool anyAddressTakenReader = false;
};

} // namespace bennett
