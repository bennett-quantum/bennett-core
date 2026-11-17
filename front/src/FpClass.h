// `llvm.is.fpclass` on the bits of a floating-point value, as LLVM's x86-64 back end computes it
// (TargetLowering::expandIS_FPCLASS in LLVM 18), so that a class test gives what the native program gives
// on every encoding, the x87 encodings no arithmetic produces included.
//
// The back end first tests the complement of the mask when LLVM's invertFPClassTestIfSimpler says it is
// simpler. Then, unless the function is `strictfp`, three masks are floating-point comparisons: "zero"
// (`x == 0`), "NaN" (`x` unordered with itself) and "zero or NaN" (unordered or equal to 0). Everything else
// is tested on the integer bits: for long double, "zero or subnormal" (both signs of both) is "the exponent
// is 0", which takes in a pseudo-denormal; "NaN" (both kinds) also takes in every encoding whose integer bit
// equals "the exponent is 0" (the unsupported ones and the pseudo-denormals); a normal number needs its
// integer bit. An x87 comparison reads a pseudo-denormal as a number and an unsupported encoding as
// unordered, so those masks are written here as what the comparison gives, on the bits.
//
// Measured: every one of the 1,024 masks on 48 encodings, every class of both signs, with and without
// `strictfp`, against clang 18's native code (`front/tests/tests/fp_class.rs`).
#pragma once

#include <cstdint>

namespace llvm {
class IRBuilderBase;
class Type;
class Value;
} // namespace llvm

namespace bennett {

// Whether the value whose integer bits are `bits` (of floating-point type `type`) is in one of the classes
// of `mask` (bit 0 signalling NaN, 1 quiet NaN, 2 -inf, 3 -normal, 4 -subnormal, 5 -0, 6 +0, 7 +subnormal,
// 8 +normal, 9 +inf), in a function that is `strictfp` or not.
llvm::Value *classTest(llvm::IRBuilderBase &b, llvm::Value *bits, llvm::Type *type, uint64_t mask, bool strictFP);

} // namespace bennett
