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

namespace llvm {
class CallBase;
class DominatorTree;
} // namespace llvm

namespace bennett {

// Warns, by name and location, about the writes of the applied call `call` that later code reads.
void warnUndoneWrites(const llvm::CallBase &call, const llvm::DominatorTree &dominators);

} // namespace bennett
