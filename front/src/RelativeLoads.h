// LLVM's relative loads, which clang's optimiser makes of a table of pointers to constants in position-independent
// code (a `switch` that chooses a string, a `static const char *const` table): `llvm.load.relative.iN(p, off)` is
// `p + sext(load i32 (p + off))` (LLVM Language Reference, "llvm.load.relative"). The device program has no such
// intrinsic, so each call is written out as those instructions. Ordinary C; nothing here concerns qubits.
#pragma once

namespace llvm {
class Module;
} // namespace llvm

namespace bennett {

// Replaces every call of `llvm.load.relative.*` by the address computation, the 32-bit load, the sign extension and
// the sum, and removes the intrinsic's declarations. Run after the last LLVM pass, before the writer.
void lowerRelativeLoads(llvm::Module &module);

} // namespace bennett
