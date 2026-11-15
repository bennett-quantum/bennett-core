// `setjmp` and `longjmp`: the C library's and clang's built-in forms, onto the device program's own `setjmp`
// instruction and `longjmp` terminator (docs/DESIGN.md, "Non-local jumps").
#pragma once

namespace llvm {
class Module;
} // namespace llvm

namespace bennett {

// The internal functions the emitter writes as the device instructions: `i32 __bennett.setjmp(ptr)` (returns
// twice) and `void __bennett.longjmp(ptr, i32)` (does not return). C cannot name them.
inline constexpr const char *deviceSetjmp = "__bennett.setjmp";
inline constexpr const char *deviceLongjmp = "__bennett.longjmp";

// Rewrites, after the optimiser:
// - calls of the undefined C library functions `setjmp`, `_setjmp` into `__bennett.setjmp`, and `longjmp`,
//   `_longjmp`, `siglongjmp`, `__longjmp_chk` into `__bennett.longjmp` with C's value (0 becomes 1);
// - `sigsetjmp`/`__sigsetjmp(buf, savemask)` into `__bennett.setjmp`, followed where `savemask` is not 0 by
//   the C library's `__sigsetjmp_tail(buf, result)`, as musl's x86-64 `sigsetjmp` does: it saves the signal
//   mask on the first return and restores it on a jump (musl provides it; without it the call is refused as
//   a call with no definition, which `rt/musl/link.py` resolves);
// - clang's `llvm.eh.sjlj.setjmp(buf)` into `__bennett.setjmp` and `llvm.eh.sjlj.longjmp(buf)` into
//   `__bennett.longjmp(buf, 1)`;
// - `llvm.frameaddress(0)` into the stack pointer at the function's entry (one per frame and constant
//   through it), which `__builtin_setjmp` stores; a caller's frame address is refused by name.
// A program's own definition of one of these names is an ordinary function and left alone. Taking the
// address of a library declaration, or calling it through another prototype, is refused by name.
void lowerJumps(llvm::Module &module);

} // namespace bennett
