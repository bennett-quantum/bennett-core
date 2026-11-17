// The subnormal mode a function promises (`denormal-fp-math`, and `denormal-fp-math-f32` for float). SoftFloat
// computes IEEE subnormals: it neither flushes a subnormal result to zero nor reads a subnormal operand as
// zero. So an operation whose result depends on the mode, in a function that promises any mode but IEEE
// (`preserve-sign`, `positive-zero`, `dynamic`), is refused by name, rather than giving IEEE results where
// the native program, run in the environment it promises (x86-64 MXCSR DAZ/FTZ), flushes. Route past the
// refusal: the floating-point environment carried through SoftFloat's state.
//
// Measured: `-fdenormal-fp-math=preserve-sign` and `-fdenormal-fp-math-f32=preserve-sign` against native code
// run with DAZ and FTZ set, at O0 and O2 (`front/tests/tests/attributes_independent.rs`).
#pragma once

namespace llvm {
class Instruction;
} // namespace llvm

namespace bennett {

// Refuses `inst` if its result depends on the subnormal mode and its function promises a mode other than
// IEEE for one of the floating-point types it reads or makes.
void checkDenormalMode(const llvm::Instruction &inst);

} // namespace bennett
