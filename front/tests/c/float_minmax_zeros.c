/* float_minmax_zeros.c: fmin and fmax (float and double) on every pair of a grid of signed zeros,
 * infinities, NaNs, subnormals and numbers. Natively they are SSE's minsd/maxsd with a NaN test, which give
 * the first of two equal operands: fmin(+0, -0) is +0 and fmax(-0, +0) is -0. The device once ordered -0
 * below +0 (found by an independent test pass). A NaN result counts as one value (which NaN is unspecified).
 * The result bits are folded into a hash, which must equal the native build's (EXPECTED, taken from it with
 * -DPRINT). Returns 0, or 1 if the hash differs. */
#ifdef PRINT
#include <stdio.h>
#endif
#define EXPECTED 10575665392763852723ULL

typedef unsigned long long u64;

static u64 h = 1469598103934665603ULL;
static void mix(u64 v) { h = (h ^ v) * 1099511628211ULL; }

static u64 dbits(double x) {
  u64 b;
  __builtin_memcpy(&b, &x, 8);
  return x != x ? 1 : b;
}
static u64 fbits(float x) {
  unsigned b;
  __builtin_memcpy(&b, &x, 4);
  return x != x ? 1 : b;
}

volatile double grid[] = {0.0, -0.0, 1.0, -1.0, 2.5, -2.5, __builtin_inf(), -__builtin_inf(),
                          __builtin_nan(""), -__builtin_nan(""), 4.9e-324, -4.9e-324};

int main(void) {
  unsigned n = sizeof grid / sizeof grid[0];
  for (unsigned i = 0; i < n; i++) {
    for (unsigned j = 0; j < n; j++) {
      double x = grid[i], y = grid[j];
      float f = (float)x, g = (float)y;
      mix(dbits(__builtin_fmin(x, y)));
      mix(dbits(__builtin_fmax(x, y)));
      mix(fbits(__builtin_fminf(f, g)));
      mix(fbits(__builtin_fmaxf(f, g)));
    }
  }
#ifdef PRINT
  printf("%lluULL\n", h);
#endif
  return h != EXPECTED;
}
