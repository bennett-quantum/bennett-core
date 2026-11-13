/* empty_asm.c: inline assembly with an empty template, used as an optimisation barrier. It executes no
 * instruction: an output holds the input tied to it, an in-out memory operand keeps its value. Returns 0,
 * or the number of the first check that failed. Several operands each, so that reading the wrong one
 * gives another value. */
__attribute__((noinline)) long hide(long x) {
  __asm__("" : "+r"(x));
  return x;
}

int main(void) {
  long a = 5, b = 7, c = 11;
  __asm__ volatile("" ::: "memory");
  __asm__ volatile("" : : "r"(a), "r"(b));
  if (hide(-3) != -3) return 1;
  long out;
  __asm__("" : "=r"(out) : "r"(b), "0"(c)); /* tied to the second input */
  if (out != 11) return 2;
  long m = 41, n = 43;
  __asm__("" : "+m"(m)); /* memory in and out: unchanged */
  if (m != 41) return 3;
  __asm__ volatile("" : "+g"(n) : : "memory"); /* register or memory, in and out */
  if (n != 43) return 4;
  long p = 1, q = 2;
  __asm__("" : "=r"(p), "=r"(q) : "0"(c), "1"(a)); /* two outputs, crossed inputs */
  if (p != 11 || q != 5) return 5;
  int narrow = -9;
  __asm__("" : "+r"(narrow));
  if (narrow != -9) return 6;
  char bytes[4] = {1, 2, 3, 4};
  __asm__ volatile("" : : "r"(bytes) : "memory");
  if (bytes[3] != 4) return 7;
  __asm__ volatile("" : "=g"(n) : "0"(n + 1)); /* the output receives its tied input */
  if (n != 44) return 8;
  return 0;
}
