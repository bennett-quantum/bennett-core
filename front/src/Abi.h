// Calls the device program has no instruction for, rewritten into calls it has, before the optimiser runs.
#pragma once

namespace llvm {
class Module;
} // namespace llvm

namespace bennett {

// Calls through a prototype that differs from the callee's definition (an unprototyped K&R call, a
// redeclared library function, a cast function pointer) become calls with the callee's own prototype, made
// to read what a native x86-64 System V call reads: each parameter from the register or stack slot where
// the caller's prototype put an argument, a result from where the callee's put it. Extra arguments are
// dropped; an argument the callee never reads may be missing; narrower reads take the low bits. Where the
// callee or the caller would read bits the other side does not set (the upper half of a 32-bit value, a
// missing argument that is read, an integer from a floating-point register), the convention determines no
// value: a direct call is refused by name; a narrow argument whose extension the callee relies on is
// checked when the call runs, and stops with the named fault `unreachable` when it is not the extension
// of its low bits. A call through a pointer compares the pointer with each function whose address is
// taken and whose prototype differs but is reconciled, and calls it so (see lowerCallThroughPointer).
// Calls of declarations are left alone (refused later). Must run before lowerVariadics.
void lowerMismatchedCalls(llvm::Module &module);

// Variadic functions and calls. The x86-64 System V convention is emulated in memory: clang has already
// lowered `va_arg` inline against it, so the device program need only give it the memory it reads.
//
// A variadic function gets one more parameter, the address of an area the caller fills: a register save
// area (6 general registers, 8 vector registers; 176 bytes) followed by the stack arguments. The caller
// places each variadic argument where the x86-64 psABI and LLVM's backend (CC_X86_64_C) put it: a
// register while one is free, counting the registers the named arguments use, otherwise the next stack
// slot of 8 bytes, or of 16 for an `i128`, or of the argument's alignment for a `byval` struct, which is
// copied there. (They differ in one case, an `i128` when one register is left: the psABI, which the
// callee's va_arg follows, leaves that register to the next argument; the backend skips it.) `va_start` points the `va_list`
// at that area with the offsets of the first free registers. The callee's inline `va_arg` then reads
// exactly what a native call would have passed, whichever way clang classified each argument. Only
// placing everything on the stack would be wrong: clang passes a 16-byte-aligned struct that fits in two
// registers as two plain `i64`s, and reads it from the stack at a 16-byte boundary.
//
// `va_copy` becomes a copy of the 24-byte `va_list`, and `va_end` is dropped. Calls through another
// prototype are lowerMismatchedCalls' (run first). Must run before lowerParameterAttributes, which would
// drop the `byval` of a variadic argument.
void lowerVariadics(llvm::Module &module);

// Results that are aggregates of integer and floating-point fields of at most 128 bits in all (`{ i64, i64 }`,
// how clang returns a small struct in two registers; SoftFloat's `extFloat80_t`) come back as one integer,
// the fields side by side from bit 0 in order, so no memory is written and code under qubit control can
// call such a function. Other aggregate results (with a pointer, or wider), results that are vectors, and
// parameters that are vectors (GCC's vector extensions): they go through memory. Each
// function that returns one takes the address of its result as a new first parameter and stores it
// there, an aggregate field by field (padding is neither written nor read, as in registers); each call
// allocates the result in the caller's entry block and reads it back. A vector argument is copied into
// the caller's frame and passed by address. Calls through pointers are rewritten alike, so they still
// match. Run after lowerVariadics, which counts the registers of the parameters as clang saw them.
void lowerValuesInMemory(llvm::Module &module);

} // namespace bennett
