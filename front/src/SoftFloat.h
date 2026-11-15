// Floating point as library code. The device program has no floating-point values, so the front does what
// LLVM's soft-float legalisation does in a back end for a machine without floating-point hardware (which
// the front cannot use: it works on SelectionDAG), as a pass of its own over LLVM IR, in three steps:
//
// 1. lowerFloatOperations: every operation on float, double and x86_fp80 (long double) becomes a call of
//    the compiler runtime-call interface, with its floating-point signature: the libgcc / compiler-rt
//    names (`__adddf3`, `__ltsf2`, `__fixdfsi`, `__mulxf3`, `__extenddfxf2`, ...) and, for what has none,
//    the C library names of LLVM's RuntimeLibcalls.def (`sqrt`, `fma`, `floor`, `fmin`, `sqrtl`, ...).
//    A conversion to an integer narrower than the routines' goes through the narrowest that LLVM's x86-64
//    back end would use, which decides what an out-of-range value gives. Comparisons follow
//    TargetLowering::softenSetCCOperands: which calls each `fcmp` predicate needs, and how their results
//    are compared with 0. `fneg`, `fabs`, `copysign` and `is.fpclass` are bit operations, written
//    inline; `fmuladd` is a multiplication and an addition, unfused, as x86-64 without FMA computes it
//    natively; `minimum`, `maximum`, `lrint` and `lround` are compositions. Vector operations are done
//    element by element.
// 2. The runtime functions the program needs are linked from a bitcode file that defines the interface
//    (by default `float-runtime.bc` next to bennett-front: Berkeley SoftFloat 3e, `rt/softfloat`), with
//    LinkOnlyNeeded. Every definition of the runtime is renamed `bennett.float.<name>` first and is
//    internal after linking: a program's own function or declaration of the same name is never
//    redirected to the runtime, and LLVM's optimiser sees the whole runtime, inlines it and folds its
//    never-written globals (the rounding mode, extF80's rounding precision) to constants.
// 3. floatsToIntegers: every floating-point type in the module, the runtime's included, becomes the
//    integer of its width (`float` i32, `double` i64, `x86_fp80` i80, ...), which has the same size and
//    alignment (checked against the data layout, with every struct field's offset, for each type
//    rewritten: an internal error if not): signatures, calls direct and indirect, loads, stores,
//    allocations, globals, constants (their bit patterns), select, phi, aggregates and vectors. A bitcast
//    between a floating-point value and an integer becomes nothing. Runs after lowerVariadics, which
//    places float and double arguments in the vector-register save area and long double ones on the
//    stack, and lowerValuesInMemory.
//
// Refused by name: `frem` (fmod) and the intrinsics of C library functions (sin, exp, pow, ...), which
// come with the C library; the fused multiply-add of long double (`fmal`, `llvm.fma.f80`: SoftFloat has
// none for the 80-bit format); operations on `half`, `bfloat`, `fp128` and `ppc_fp128` values (moving
// them is fine); conversions between floating point and integers wider than 128 bits; constrained
// intrinsics; atomic floating-point updates. A program without floating point is unchanged and no
// runtime is linked.
#pragma once

#include <string>

namespace llvm {
class Module;
} // namespace llvm

namespace bennett {

// Steps 1 and 2: `runtime` is the bitcode file defining the interface, read only if it is needed.
// Returns whether it was linked.
bool lowerFloatOperations(llvm::Module &module, const std::string &runtime);

// Step 3.
void floatsToIntegers(llvm::Module &module);

// Evaluates each call of a runtime function whose arguments are constants, with LLVM's Evaluator, and
// simplifies what uses the result: LLVM's optimiser folds such a call only where it inlines the routine,
// and a rotation's angle must be a constant. A rotation's angle computed by a function of the program
// that only reads memory is inlined first, up to 8 levels. Returns whether anything changed.
bool foldConstantCalls(llvm::Module &module);

// Stops with an internal error naming it if any floating-point type is left anywhere in the module.
void checkNoFloatingPoint(const llvm::Module &module, const char *stage);

} // namespace bennett
