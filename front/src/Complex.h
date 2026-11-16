// The complex multiply and divide helpers. clang calls `__mulsc3`, `__divsc3`, `__muldc3`, `__divdc3`,
// `__mulxc3` and `__divxc3` for `_Complex float`, `double` and `long double` products and quotients (the
// C Annex G recovery of infinities and NaNs is in them); natively the compiler runtime defines them. Here
// they come from libgcc's own sources (`rt/libgcc`, vendored unmodified), compiled to a bitcode
// file (by default `complex-runtime.bc` next to bennett-front).
//
// Unlike the floating-point runtime, the helpers are ordinary C on floating-point values: they are linked
// into the program before its floating-point operations are lowered, and lowered with it. Only the
// helpers the program calls and does not define itself are linked (as a static linker pulls archive
// members), and they are internal afterwards. All six are self-contained, including long-double division
// (`rt/libgcc/README.md`).
#pragma once

#include <string>

namespace llvm {
class Module;
} // namespace llvm

namespace bennett {

// Links the helpers `module` calls without defining from the bitcode file `runtime`, read only if one is
// needed. Returns whether any was linked.
bool linkComplexHelpers(llvm::Module &module, const std::string &runtime);

} // namespace bennett
