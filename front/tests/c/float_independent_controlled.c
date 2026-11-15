/* Correlated cancellation and rounding boundaries under qubit control, checked
 * as full branch states. */
#include "bennett.h"
typedef unsigned long long u64;
static u64 fused(unsigned char k) {
  long double a = 1.L + (k + 1) * 0x1p-63L;
  long double r = __builtin_fmal(a, 1.L - 0x1p-63L, -1.L);
  u64 m;
  unsigned short e;
  __builtin_memcpy(&m, &r, 8);
  __builtin_memcpy(&e, (char *)&r + 8, 2);
  return m ^ e;
}
static u64 wide(unsigned char k) {
  long double x = 0x1p64L + ((int)k - 8) * 2.L;
  unsigned __int128 n = (unsigned __int128)x;
  long double y = (long double)(n + 1);
  u64 m;
  __builtin_memcpy(&m, &y, 8);
  return m ^ (u64)(n >> 64);
}
static u64 small(unsigned char k) {
  _Float16 a = (_Float16)(1.L + k * 0x1p-10L), b = 0x1p-11, c = -1;
  __bf16 x = (__bf16)(1.f + k * 0x1p-7f), y = 0x1p-8, z = -1;
  _Float16 h = (a + b) + c;
  __bf16 v = (x + y) + z;
  unsigned short hb, bb;
  __builtin_memcpy(&hb, &h, 2);
  __builtin_memcpy(&bb, &v, 2);
  return hb | ((u64)bb << 16) | ((u64)(a + b > a) << 32) |
         ((u64)(x + y > x) << 33);
}
#ifndef NATIVE
u64 ratio(void) {
  _Bool q[4] = {0};
  for (int i = 0; i < 4; i++)
    bennett_h(&q[i]);
  unsigned char k = 0;
  for (int i = 0; i < 4; i++)
    k |= (unsigned char)(q[i] << i);
  u64 f = fused(k), w = wide(k), s = small(k);
  u64 r = bennett_measure(&f);
  r ^= bennett_measure(&w);
  r ^= bennett_measure(&s);
  return r ^ bennett_measure(q);
}
#else
int printf(const char *, ...);
int main(void) {
  for (unsigned char k = 0; k < 16; k++)
    printf("%llu %llu %llu\n", fused(k), wide(k), small(k));
}
#endif
