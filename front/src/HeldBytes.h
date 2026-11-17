// Memory written under a qubit (docs/DESIGN.md, "Memory written under a qubit").
//
// A store into memory at a definite address whose value, or whether it runs, depends on a qubit makes the bytes
// it writes hold qubits, and keeps the value in superposition there. The front names those bytes by their object
// (a global variable or the memory a pointer argument points to) and their offsets, and gives them a qubit
// variable of their width, a *shadow*, for the stretch of the function in which they hold qubits (a *span*):
//
// - at the start, before the first such write, an ordinary load reads the bytes' old value v, and three CNOTs
//   per bit exchange the fresh shadow and the bytes (`bennett.exchange`): the shadow holds v, a value the front
//   knows, and the bytes hold qubits at 0;
// - in the span, every access to those bytes is an access to the shadow, so the qubit variables' forms apply:
//   code under a qubit without gates is one `apply` (RegionResults.h), its loops included; other stores are
//   XORed in, inverted in place, or exchanged into a fresh register that keeps the old value (Control.cpp);
// - at its end, before a return and before a `bennett_measure_bytes` of any of those bytes, the same exchange
//   puts the value into the bytes and returns the shadow to 0. A measurement of whole bytes makes them ordinary
//   memory again; the others keep holding qubits, and ordinary code in the function that reads them is refused
//   (measure first).
//
// Written as a whole, this is the exchange form of a store: the new value is applied into a fresh register,
// exchanged with the bytes by gates on address-named bits, and the register is cleared because the old value
// is known. For a local variable of the function the shadow takes the old value by XOR instead, the stack bytes
// keep it, and the exchange happens only before a measurement of all the bytes: its stack memory never holds
// qubits after it returns.
//
// The front assumes the bytes hold no qubits where a span starts. If an earlier call left them holding qubits,
// the ordinary load there is the fault `qubit_memory_access` (a named fault, never a wrong value).
#pragma once

#include <set>
#include <vector>

namespace llvm {
class BasicBlock;
class Function;
class Instruction;
class Value;
} // namespace llvm

namespace bennett {

class ControlAnalysis;

// The metadata that marks a front temporary qubit variable as the shadow of bytes in memory; its string describes
// the bytes.
inline constexpr const char *heldMetadata = "bennett.held";

// Whether `value` is a call of the front's exchange: `bennett.exchange(ptr shadow, ptr address, i64 after)`
// exchanges every bit of the shadow with the bit of memory at the same position from `address` (three CNOTs per
// bit); `after` is the value the shadow holds afterwards (the bytes' old value, or 0).
bool isExchange(const llvm::Value *value);

// Makes the bytes the stores `writes` may write hold qubits, as above. `start` is where the span starts at the
// latest (before the code under a qubit, or before a store of a qubit-dependent value); `region` the blocks of
// that code, the only place where a position that is not constant may be followed into the shadow. Refuses by
// name what it cannot do.
void holdWrites(llvm::Function &function, const std::vector<llvm::Instruction *> &writes, llvm::Instruction *start,
                const std::set<const llvm::BasicBlock *> &region, ControlAnalysis &analysis);

// Finds a store of a value that depends on a qubit into memory outside every qubit variable, in code no qubit
// controls, and holds its bytes. Returns whether it found one.
bool holdStraightWrite(llvm::Function &function, ControlAnalysis &analysis);

// After outlining: a store into part of a shadow in the control program becomes a store of the whole shadow
// (the other bytes kept); a position that is not constant in a shadow, outside code applied whole, is refused.
// Returns whether it changed anything.
bool finishHeld(llvm::Function &function, ControlAnalysis &analysis);

} // namespace bennett
