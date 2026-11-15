/* float_leftovers_controlled.c: the routines beyond SoftFloat under quantum control: fmal and a conversion
 * to and from a 128-bit integer, applied coherently to a superposition.
 * ratio()'s comment states the state just before its first measurement; floating.rs checks it against what
 * the native build prints with -DNATIVE. */
#include "bennett.h"

typedef unsigned long long u64;

static u64 low(long double x) {
  u64 r;
  __builtin_memcpy(&r, &x, 8);
  return r;
}

/* For k < 16: fmal(k, 1/3, 0.1)'s low 64 bits; (__int128)(k * 2^70 / 3.0) + 12345 converted back to double. */
static u64 fused(unsigned char k) { return low(__builtin_fmal((long double)k, 1.0L / 3, 0.1L)); }
static u64 wide(unsigned char k) {
  __int128 t = (__int128)((double)k * 0x1p70 / 3.0);
  double d = (double)(t + 12345);
  u64 r;
  __builtin_memcpy(&r, &d, 8);
  return r;
}

/* The sum over k < 16 of |k, fused(k), wide(k)>/4 in (q, f, w), where q holds k's bits one per byte. */
#ifndef NATIVE
u64 ratio(void) {
  _Bool q[4] = {0};
  for (int i = 0; i < 4; i++)
    bennett_h(&q[i]);
  unsigned char k = 0;
  for (int i = 0; i < 4; i++)
    k |= (unsigned char)(q[i] << i);
  u64 f = fused(k);
  u64 w = wide(k);
  u64 r = bennett_measure(&f);
  r ^= bennett_measure(&w);
  return r ^ bennett_measure(q);
}
#else
int printf(const char *, ...);
int main(void) {
  for (unsigned char k = 0; k < 16; k++) {
    printf("%llu %llu\n", fused(k), wide(k));
  }
  return 0;
}
#endif
