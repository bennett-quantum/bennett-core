// Calls the device program has no instruction for, rewritten into calls it has, before the optimiser runs.
#pragma once

namespace llvm {
class Module;
} // namespace llvm

namespace bennett {

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
// `va_copy` becomes a copy of the 24-byte `va_list`, and `va_end` is dropped. A call with a variadic
// type of a function that is not variadic (an unprototyped K&R declaration) becomes a plain call when
// the arguments the function takes match its parameters exactly; the native call passes the rest in
// registers or on the stack, where the function never reads them. Must run before
// lowerParameterAttributes, which would drop the `byval` of a variadic argument.
void lowerVariadics(llvm::Module &module);

// Results that are aggregates (`{ i64, i64 }`, how clang returns a small struct in two registers) or
// vectors, and parameters that are vectors (GCC's vector extensions): they go through memory. Each
// function that returns one takes the address of its result as a new first parameter and stores it
// there, an aggregate field by field (padding is neither written nor read, as in registers); each call
// allocates the result in the caller's entry block and reads it back. A vector argument is copied into
// the caller's frame and passed by address. Calls through pointers are rewritten alike, so they still
// match. Run after lowerVariadics, which counts the registers of the parameters as clang saw them.
void lowerValuesInMemory(llvm::Module &module);

} // namespace bennett
