/* small_results_controlled.c: functions that return a small struct, called under quantum control. clang
 * returns such a struct as an LLVM aggregate ({ i64, i64 }, { double, i32 }); the front carries it as one
 * integer, so no memory is written and the calls run coherently. ratio()'s comment states the state just
 * before its first measurement; floating.rs checks it against what the native build prints with -DNATIVE. */
#include "bennett.h"

typedef unsigned long long u64;

struct pair {
  long long a, b;
};
struct mixed {
  double d;
  int i;
};

__attribute__((noinline)) struct pair split(unsigned char k) {
  struct pair p = {(long long)k * 0x123456789, -((long long)k << 40)};
  return p;
}
__attribute__((noinline)) struct mixed scaled(unsigned char k) {
  struct mixed m = {k * 0.1, k * 7 - 50};
  return m;
}

static u64 first(unsigned char k) {
  struct pair p = split(k);
  return (u64)p.a + (u64)p.b * 3;
}
static u64 second(unsigned char k) {
  struct mixed m = scaled(k);
  u64 r;
  __builtin_memcpy(&r, &m.d, 8);
  return r ^ (u64)(long long)m.i;
}

/* The sum over k < 16 of |k, first(k), second(k)>/4 in (q, f, s), where q holds k's bits one per byte. */
#ifndef NATIVE
u64 ratio(void) {
  _Bool q[4] = {0};
  for (int i = 0; i < 4; i++)
    bennett_h(&q[i]);
  unsigned char k = 0;
  for (int i = 0; i < 4; i++)
    k |= (unsigned char)(q[i] << i);
  u64 f = first(k);
  u64 s = second(k);
  u64 r = bennett_measure(&f);
  r ^= bennett_measure(&s);
  return r ^ bennett_measure(q);
}
#else
int printf(const char *, ...);
int main(void) {
  for (unsigned char k = 0; k < 16; k++) {
    printf("%llu %llu\n", first(k), second(k));
  }
  return 0;
}
#endif
