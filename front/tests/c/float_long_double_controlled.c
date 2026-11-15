/* float_long_double_controlled.c: long double under quantum control. ratio()'s comment states the state
 * just before its first measurement; floating.rs checks it against what the native build prints with
 * -DNATIVE (Rust has no 80-bit floating point). */
#include "bennett.h"

typedef unsigned long long u64;

static u64 low(long double x) {
  u64 r;
  __builtin_memcpy(&r, &x, 8);
  return r;
}
static u64 high(long double x) {
  unsigned short r;
  __builtin_memcpy(&r, (const char *)&x + 8, 2);
  return r;
}

/* The sum over k < 16 of |k, low(y_k), high(y_k), k * 0.3L > 2.0L>/4 in (q, lo, hi, big), where q holds
 * k's bits one per byte and y_k = k / 3.0L + 0.1L: a computation on long doubles applied coherently, and
 * a gate controlled by a comparison of long doubles. */
#ifndef NATIVE
u64 ratio(void) {
  _Bool q[4] = {0};
  for (int i = 0; i < 4; i++)
    bennett_h(&q[i]);
  unsigned char k = 0;
  for (int i = 0; i < 4; i++)
    k |= (unsigned char)(q[i] << i);
  long double y = (long double)k / 3.0L + 0.1L;
  u64 lo = low(y);
  u64 hi = high(y);
  _Bool big = 0;
  if ((long double)k * 0.3L > 2.0L)
    bennett_x(&big);
  u64 r = bennett_measure(&lo);
  r ^= bennett_measure(&hi);
  r ^= bennett_measure(&big);
  return r ^ bennett_measure(q);
}
#else
int printf(const char *, ...);
int main(void) {
  for (unsigned k = 0; k < 16; k++) {
    long double y = (long double)k / 3.0L + 0.1L;
    printf("%llu %llu %d\n", low(y), high(y), (long double)k * 0.3L > 2.0L);
  }
  return 0;
}
#endif
