/* A call whose prototype differs from the callee's definition (here through a cast; in c-testsuite 00025 through
 * a wrong declaration of strlen). LLVM keeps the callee as a constant, but CallBase::getCalledFunction() is null
 * when the types differ, so the refusal "a call through a mismatched prototype" in front/src/Emit.cpp is never
 * reached; front emits `call_indirect fn(i32) -> i32 1(...)` and the run ends in the named fault bad_indirect.
 * Native (x86-64): exit 0. Device: fault bad_indirect. Named, not silent; the intended refusal is dead code. */
long f(long x) { return x; }
int main(void) { return ((int (*)(int))f)(5) - 5; }
