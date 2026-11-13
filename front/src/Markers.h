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
};

struct Marker {
  MarkerKind kind;
  std::string mnemonic; // the device program's gate mnemonic: x h z s sdg t tdg rx ry rz; "measure"
};

// The marker a function is, if it is one. Only the name is consulted; checkMarkers has checked the rest.
std::optional<Marker> markerOf(const llvm::Function *callee);

// The marker a call calls, if any.
std::optional<Marker> markerCall(const llvm::Value *value);

// Checks every function named bennett_*: it must be a known marker, only declared, with the marker's
// signature, and without attributes that would let LLVM delete, merge or speculate its calls.
void checkMarkers(const llvm::Module &module);

} // namespace bennett
