/* float_leftovers_controlled.c: the routines beyond SoftFloat under quantum control: fmal, a conversion to
 * and from a 128-bit integer, _Float16 and __bf16 arithmetic, applied coherently to a superposition.
 * ratio()'s comment states the state just before its first measurement; floating.rs checks it against what
 * the native build prints with -DNATIVE. */
#include "bennett.h"

typedef unsigned long long u64;

static u64 low(long double x) {
  u64 r;
  __builtin_memcpy(&r, &x, 8);
  return r;
}
static u64 bits16(const void *x) {
  unsigned short r;
  __builtin_memcpy(&r, x, 2);
  return r;
}

/* For k < 16: fmal(k, 1/3, 0.1)'s low 64 bits; the low 64 bits of (__int128)(k * 2^70 / 3.0) converted back
 * to double; the bits of k / 3 in _Float16 and of k * 0.1 in __bf16, side by side. */
static u64 fused(unsigned char k) { return low(__builtin_fmal((long double)k, 1.0L / 3, 0.1L)); }
static u64 wide(unsigned char k) {
  __int128 t = (__int128)((double)k * 0x1p70 / 3.0);
  double d = (double)(t + 12345);
  u64 r;
  __builtin_memcpy(&r, &d, 8);
  return r;
}
static u64 small(unsigned char k) {
  _Float16 h = (_Float16)k / (_Float16)3;
  __bf16 b = (__bf16)k * (__bf16)0.1f;
  return bits16(&h) | bits16(&b) << 16;
}

/* The sum over k < 16 of |k, fused(k), wide(k), small(k)>/4 in (q, f, w, s), where q holds k's bits one per
 * byte. */
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
  u64 s = small(k);
  u64 r = bennett_measure(&f);
  r ^= bennett_measure(&w);
  r ^= bennett_measure(&s);
  return r ^ bennett_measure(q);
}
#else
int printf(const char *, ...);
int main(void) {
  for (unsigned char k = 0; k < 16; k++) {
    printf("%llu %llu %llu\n", fused(k), wide(k), small(k));
  }
  return 0;
}
#endif
