// Which memory two pointers of a function may share, across the calls of the module, for memory written under a
// qubit (docs/DESIGN.md, "Memory written under a qubit"). The front keeps one shadow per object (HeldBytes.h); it
// must know where another pointer, or a call, may reach the bytes a shadow holds.
//
// An object is a global variable that is not constant, a local variable, or a pointer argument. What a pointer
// argument may point into is the union, over the calls of its function in the module, of what each call passes;
// for a function the host may call (one with external linkage), or one called through a pointer, it is anything.
// A pointer the front cannot follow (loaded from memory, returned by a call) may point into any object whose
// address escapes (LLVM's capture tracking). An argument never points into a local variable of its own call, and
// a `restrict` argument shares no bytes with another pointer unless its address escapes (a pointer loaded from
// where a copy of it was stored is based on it). A call of `bennett_measure_bytes` reads the memory it measures.
// When unsure, the answer is "may share".
#pragma once

#include <map>
#include <set>

namespace llvm {
class Argument;
class CallBase;
class Function;
class Module;
class Value;
} // namespace llvm

namespace bennett {

class Aliasing {
public:
  explicit Aliasing(const llvm::Module &module);

  // Whether memory reached through the pointers `a` and `b` of the function `function` may share bytes.
  bool mayShare(const llvm::Function &function, const llvm::Value *a, const llvm::Value *b) const;
  // Whether `call` (in `function`) may read or write memory reached through `pointer`, directly or through the
  // functions it calls.
  bool mayTouch(const llvm::Function &function, const llvm::CallBase &call, const llvm::Value *pointer) const;
  // Whether `call` (in `function`) may read or write memory other than the frames of the functions it calls:
  // memory that exists before the call starts. A local variable of a function other than `function` exists
  // only in a frame of the call's own (a fresh one for each call), so a callee whose only accesses are to its
  // own local variables reaches none.
  bool reachesOutsideFrames(const llvm::Function &function, const llvm::CallBase &call) const;

  // A set of objects, for a caller that collects the objects several pointers may point into (Classify.h).
  using Objects = std::set<const llvm::Value *>;
  // Adds to `into` the objects `pointer` may point into. A `restrict` argument stands for what its calls pass, not
  // for an object of its own: what is written through it outlives the call.
  void addObjectsOf(const llvm::Value *pointer, Objects &into) const;
  // Whether memory reached through `pointer` (in `function`) may share bytes with one of `objects`, a `restrict`
  // argument standing for what its calls pass.
  bool mayReach(const llvm::Function &function, const llvm::Value *pointer, const Objects &objects) const;

private:
  // What an object set may hold besides named objects (globals and local variables).
  static const llvm::Value *const escaped; // memory whose address escapes
  static const llvm::Value *const anything;

  std::map<const llvm::Argument *, Objects> arguments;
  std::map<const llvm::Function *, Objects> touched; // what each defined function may access
  mutable std::map<const llvm::Value *, bool> captured;

  // The objects `pointer` may point into, its function's arguments resolved by what they may point to.
  Objects objectsOf(const llvm::Value *pointer) const;
  // `objects` with each `restrict` argument replaced by what its calls pass (anything when not known).
  Objects withoutRestrict(Objects objects) const;
  Objects touchedBy(const llvm::CallBase &call) const;
  bool escapes(const llvm::Value *object) const;
  bool meet(const llvm::Function &function, const Objects &a, const Objects &b) const;
};

// Where every call of `function` in the module passes the same pointer for two of its pointer arguments, and
// nothing else can call it (local linkage, its address not taken), the second argument's uses become uses of the
// first: the memory both name is then one object. Returns whether it changed anything.
bool mergeEqualArguments(llvm::Function &function);

} // namespace bennett
