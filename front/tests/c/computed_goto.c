/* computed_goto.c: GCC's labels as values. A small stack machine dispatches through a static table of
 * label addresses, and again through a table of label differences (the position-independent idiom);
 * values live across the indirect jumps. Returns 0, or the number of the first check that failed. */
enum { PUSH, ADD, MUL, NEG, DUP, SWAP, JNZ, DEC, HALT };

__attribute__((noinline)) long run(const signed char *code) {
  static void *const table[] = {&&push, &&add, &&mul, &&neg, &&dup, &&swap, &&jnz, &&dec, &&halt};
  long stack[16];
  int sp = 0, pc = 0;
#define NEXT goto *table[code[pc++]]
  NEXT;
push:
  stack[sp++] = code[pc++];
  NEXT;
add:
  sp--;
  stack[sp - 1] += stack[sp];
  NEXT;
mul:
  sp--;
  stack[sp - 1] *= stack[sp];
  NEXT;
neg:
  stack[sp - 1] = -stack[sp - 1];
  NEXT;
dup:
  stack[sp] = stack[sp - 1];
  sp++;
  NEXT;
swap: {
  long t = stack[sp - 1];
  stack[sp - 1] = stack[sp - 2];
  stack[sp - 2] = t;
  NEXT;
}
jnz: {
  signed char to = code[pc++];
  if (stack[--sp]) pc = to;
  NEXT;
}
dec:
  stack[sp - 1]--;
  NEXT;
halt:
  return sp == 1 ? stack[0] : -999;
}

/* The same machine dispatched by offsets from a base label. */
__attribute__((noinline)) long run_relative(const signed char *code) {
  static const int offsets[] = {&&push - &&push, &&add - &&push, &&mul - &&push, &&neg - &&push,
                                &&dup - &&push,  &&swap - &&push, &&jnz - &&push, &&dec - &&push,
                                &&halt - &&push};
  long stack[16];
  int sp = 0, pc = 0;
#undef NEXT
#define NEXT goto *(&&push + offsets[code[pc++]])
  NEXT;
push:
  stack[sp++] = code[pc++];
  NEXT;
add:
  sp--;
  stack[sp - 1] += stack[sp];
  NEXT;
mul:
  sp--;
  stack[sp - 1] *= stack[sp];
  NEXT;
neg:
  stack[sp - 1] = -stack[sp - 1];
  NEXT;
dup:
  stack[sp] = stack[sp - 1];
  sp++;
  NEXT;
swap: {
  long t = stack[sp - 1];
  stack[sp - 1] = stack[sp - 2];
  stack[sp - 2] = t;
  NEXT;
}
jnz: {
  signed char to = code[pc++];
  if (stack[--sp]) pc = to;
  NEXT;
}
dec:
  stack[sp - 1]--;
  NEXT;
halt:
  return sp == 1 ? stack[0] : -999;
}

/* A label address chosen at run time, carried through a variable, and jumped to. (Not compared: LLVM
 * folds a comparison of label addresses differently at -O0 and -O2.) */
__attribute__((noinline)) int choose(int n) {
  void *where = n > 2 ? &&big : &&small;
  int r = n * 10 + 1;
  goto *where;
small:
  return r + 100;
big:
  return r + 200;
}

int main(void) {
  /* (3 + 4) * -5 */
  static const signed char arithmetic[] = {PUSH, 3, PUSH, 4, ADD, PUSH, 5, NEG, MUL, HALT};
  if (run(arithmetic) != -35) return 1;
  if (run_relative(arithmetic) != -35) return 2;
  /* countdown: push 6; loop: dec; dup; jnz loop -> 0 */
  static const signed char loop[] = {PUSH, 6, DEC, DUP, JNZ, 2, HALT};
  if (run(loop) != 0) return 3;
  if (run_relative(loop) != 0) return 4;
  if (choose(1) != 111 || choose(3) != 231) return 5;
  /* 2 * 3, swapped and negated: -(3 * 2) + 7 */
  static const signed char swapped[] = {PUSH, 2, PUSH, 3, SWAP, MUL, NEG, PUSH, 7, ADD, HALT};
  if (run(swapped) != 1 || run_relative(swapped) != 1) return 6;
  return 0;
}
