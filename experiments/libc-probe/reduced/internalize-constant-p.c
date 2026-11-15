/* Wrong result caused by the probe's LINK=internalize step, not by bennett-front: once `global` is internal, LLVM
 * proves it is never written (the increment inside __builtin_constant_p is not evaluated), folds it to 0, and
 * __builtin_constant_p(global) becomes 1. Native (global external): status 0. Device after internalising every symbol
 * but the entry: status 1. With LINK=external: status 0. */
int global;
int bump(void) { return __builtin_constant_p(global++); }
int probe(void) { return __builtin_constant_p(global); }
int (*volatile table[])(void) = {bump, probe};
int main(void) { return table[0]() | table[1](); }
