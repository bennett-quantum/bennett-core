// Applied calls. A call whose arguments depend on a qubit, to an ordinary function (one with no quantum
// operation, directly or through anything it calls), is applied whole: the applied function runs on a
// private copy of memory for each definite value of its arguments, so whatever it writes, allocates or
// frees is undone when it ends, and only its result survives (docs/DESIGN.md, "Applied calls").
//
// Natively the writes would remain. Where the front can see that they matter, it says so: a warning when
// the called function (or a function it calls directly) writes a global variable, or memory passed to it
// by pointer, that code after the call in the calling function reads, directly or through a function it
// calls directly. Writes through other pointers, and reads through calls the front cannot follow, are not
// seen: the absence of the warning proves nothing.
#pragma once

#include <map>
#include <set>

namespace llvm {
class CallBase;
class DominatorTree;
class Function;
class GlobalVariable;
class Module;
} // namespace llvm

namespace bennett {

// The memory a function may write or read that the front can name: global variables, and the memory its
// pointer parameters point to (by parameter number).
struct Footprint {
  std::set<const llvm::GlobalVariable *> globals;
  std::set<unsigned> parameters;

  bool add(const Footprint &other);
};

struct Effects {
  Footprint writes, reads;
};

// The effects of every defined function of a module, through the functions each calls directly, computed
// once for the module.
class AppliedCalls {
public:
  explicit AppliedCalls(const llvm::Module &module);

  // Warns, by name and location, about the writes of the applied call `call` that later code reads.
  void warnUndoneWrites(const llvm::CallBase &call, const llvm::DominatorTree &dominators) const;

private:
  std::map<const llvm::Function *, Effects> effects;
};

} // namespace bennett
