/* A no-output abort boundary for the device. No libc or host syscall emulation. */
_Noreturn void abort(void) { __builtin_trap(); }
