// The marker calls: ordinary external functions named `bennett_*`, recognised by name and signature.
#pragma once

#include <optional>
#include <string>

namespace llvm {
class CallBase;
class Function;
class Module;
class Value;
} // namespace llvm

namespace bennett {

enum class MarkerKind {
  Gate,     // void bennett_<gate>(_Bool *qubit)
  Rotation, // void bennett_<axis>(_Bool *qubit, double theta)
  Measure,  // unsigned long long bennett_measure(const void *variable)
  Bit,      // _Bool *bennett_bit(void *variable, unsigned bit): names one qubit of a wider variable
};

struct Marker {
  MarkerKind kind;
  std::string mnemonic; // the device program's gate mnemonic: x h z s sdg t tdg rx ry rz; "measure"; "bit"
};

// The marker a function is, if it is one. Only the name is consulted; checkMarkers has checked the rest.
std::optional<Marker> markerOf(const llvm::Function *callee);

// The marker a call calls, if any.
std::optional<Marker> markerCall(const llvm::Value *value);

// Whether `value` is a call of a marker that acts on a qubit: a gate or a rotation.
bool isGateCall(const llvm::Value *value);

// The name of the operand bundle that carries the condition of a controlled gate. `bennett-front` makes
// such gates itself from a gate under a branch on a value that depends on a qubit: the gate acts on the
// branches where the bundle's single i1 operand is 1.
inline constexpr const char *controlBundle = "bennett.control";

// Checks every function named bennett_*: it must be a known marker, only declared, with the marker's
// signature, and without attributes that would let LLVM delete, merge or speculate its calls.
void checkMarkers(const llvm::Module &module);

} // namespace bennett
