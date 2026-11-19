// Which instructions the language's own rules depend on, decided before LLVM's optimiser.
//
// LLVM's optimiser keeps what native C does. Three things that the language's rules add to C must come through it
// unchanged, and they are decided here, on the program as clang hands it over (after SROA, Passes.h):
//
// - where each access to memory that may hold qubits stands (an ordinary access of a byte that holds qubits is
//   the fault `qubit_memory_access`);
// - which calls take an argument that depends on a qubit (such a call is applied, and its writes are undone);
// - which stores store a value that depends on a qubit (such a store makes the bytes it writes hold qubits).
//
// Each instruction they name gets a class, in the metadata `bennett.class`; Passes.h seals it for every pass
// that could move, merge, copy or remove it. The classes:
//
// - read:          a load, or an atomic load, of memory that may hold qubits;
// - held store:    a store of an ordinary value into memory that may hold qubits;
// - qubit store:   a store whose value or address depends on a qubit, or that runs under a conditional jump or
//                  switch that depends on one;
// - applied call:  a call, not of a marker or of an intrinsic, with an argument that depends on a qubit (a call
//                  through a pointer included);
// - held memory:   memcpy, memmove or memset that may touch such memory, or that depends on a qubit;
// - atomic update: atomicrmw or cmpxchg on such memory, or that depends on a qubit.
//
// Memory "may hold qubits" when the front's alias analysis (Aliasing.h) says it may share bytes with an object a
// qubit store, a held-memory call or an atomic update that depends on a qubit writes, or with a global variable a
// marker acts on.
//
// "Depends on a qubit" is decided by the text of the program: a value depends on a qubit at a place if, on some
// path to that place, it was computed from a qubit. The sources are: a load of a qubit variable that is not
// known there; a read of memory that may hold qubits; the result of an applied call; a phi at the join of a
// conditional jump that depends on a qubit; a parameter of a function with gates that some call passes such a
// value, and what a function returns when it may return such a value; the result of a call whose pointer
// arguments may reach memory that may hold qubits. A qubit variable is *known* at a load exactly as the front's
// later analysis decides it (Taint.h): one ordinary value stored whole, or a measurement, or the fresh 0, reaches
// the load on every path, with nothing else written to the variable in between. A qubit variable whose address
// goes anywhere but loads, stores and markers (to a helper, into memory) is never known here, and a parameter is
// never known. With `--taint-all` no qubit variable is known, as later.
//
// This analysis runs before inlining and so must follow markers applied through a helper's parameter, which the
// front's later analysis refuses; and it sees the program before the optimiser, where the later analysis sees it
// after. Where it is coarser than the later analysis (a variable passed to a helper, a value merged with itself)
// it classifies more: the optimiser then keeps more instructions as they stand, which costs only optimisation,
// since the front decides again, after the optimiser, how each restored instruction is translated. It is not
// meant to be finer: what it calls known should be known to the later analysis too, because the optimiser keeps
// every gate between the store or measurement that made it known and the load (a gate's call takes the
// variable's address, and no load or store of the variable moves across it). One way to make it finer is known
// and not excluded: a pass that copies the store of the known value into two paths, after which the later
// analysis meets two values (docs/DESIGN.md, "Not yet").
//
// What can be neither classified nor left to the optimiser is refused by name: an `invoke` or `callbr` with an
// argument that depends on a qubit, an intrinsic other than the three memory intrinsics, and a `va_arg`, that
// may touch memory that may hold qubits.
#pragma once

#include <optional>

namespace llvm {
class Instruction;
class Module;
} // namespace llvm

namespace bennett {

enum class OperationClass { Read, HeldStore, QubitStore, AppliedCall, HeldMemory, AtomicUpdate };

// The metadata kind that carries the class of an instruction.
inline constexpr const char *classMetadata = "bennett.class";

// The class of `inst`, if it has one.
std::optional<OperationClass> classOf(const llvm::Instruction &inst);
// A short name of the class, without spaces: "read", "held-store", ...
const char *className(OperationClass kind);
// What instructions of the class are, for messages.
const char *classDescription(OperationClass kind);
// Removes the class: the front has turned the access into an access of a qubit variable (HeldBytes.h), which its
// own analyses govern from then on.
void declassify(llvm::Instruction &inst);

// Classifies every instruction of `module` (after SROA, before the optimiser) as above.
void classify(llvm::Module &module, bool taintAll);

// Splits each classified load and store of a vector of whole-byte elements into one access per element, each with
// the class of the access it came from, as LLVM's Scalarizer would split it. Runs before the Scalarizer, which
// cannot split an access it sees sealed (Passes.h).
void splitClassifiedVectors(llvm::Module &module);

} // namespace bennett
