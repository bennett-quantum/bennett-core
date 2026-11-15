/* Shape-specific calls under a qubit branch, with a phase oracle to check
 * interference as well as bits. */
#include "bennett.h"
typedef unsigned long long u64;
typedef unsigned __int128 u128;
#define NI __attribute__((noinline))
#ifndef ABI_SHAPE
#define ABI_SHAPE 0
#endif
#if ABI_SHAPE == 0
struct value {
  u64 a, b;
};
NI struct value make(unsigned k) {
  return (struct value){(u64)k << 60, 17 * k + 3};
}
static u64 checksum(struct value p) { return p.a ^ p.b; }
#elif ABI_SHAPE == 1
struct value {
  double a;
  int b;
};
NI struct value make(unsigned k) {
  return (struct value){k * 0.125, (int)k - 5};
}
static u64 checksum(struct value p) {
  u64 b;
  __builtin_memcpy(&b, &p.a, 8);
  return b ^ (unsigned)p.b;
}
#elif ABI_SHAPE == 2
struct value {
  u128 a;
};
NI struct value make(unsigned k) {
  return (struct value){((u128)(k + 1) << 110) | (9 * k + 7)};
}
static u64 checksum(struct value p) { return (u64)p.a ^ (u64)(p.a >> 64); }
#elif ABI_SHAPE == 3
struct value {
  unsigned char a[15];
};
NI struct value make(unsigned k) {
  struct value p;
  for (unsigned i = 0; i < 15; ++i)
    p.a[i] = k * (i + 1) + i;
  return p;
}
static u64 checksum(struct value p) {
  u64 h = 0;
  for (unsigned i = 0; i < 15; ++i)
    h = h * 17 + p.a[i];
  return h;
}
#elif ABI_SHAPE == 4
struct value {
  struct {
    unsigned a;
    unsigned short b;
  } a;
  _Bool b;
  unsigned char c;
};
NI struct value make(unsigned k) {
  return (struct value){{k * 1234567, k * 11}, k & 1, k + 17};
}
static u64 checksum(struct value p) {
  return p.a.a ^ ((u64)p.a.b << 32) ^ ((u64)p.b << 48) ^ p.c;
}
#elif ABI_SHAPE == 5
struct value {
  long double a;
};
NI struct value make(unsigned k) {
  return (struct value){(long double)k / 8.0L};
}
static u64 checksum(struct value p) { return (u64)(p.a * 24.0L) + 3; }
#elif ABI_SHAPE == 6
struct value {
  float a, b, c, d;
};
NI struct value make(unsigned k) {
  return (struct value){k * 0.5f, k * 2.0f, k * 4.0f, k * 8.0f};
}
static u64 checksum(struct value p) { return (u64)(p.a + p.b + p.c + p.d); }
#elif ABI_SHAPE == 7
struct value {
  const unsigned *a;
  unsigned b;
};
static const unsigned base = 123;
NI struct value make(unsigned k) { return (struct value){&base, k * 7}; }
static u64 checksum(struct value p) { return *p.a ^ p.b; }
#endif
static u64 compute(unsigned k) { return checksum(make(k)); }
#ifndef NATIVE
u64 ratio(void) {
  unsigned char q = 0;
  u64 y = 0;
  _Bool phase = 0;
  for (unsigned i = 0; i < 3; ++i)
    bennett_h(bennett_bit(&q, i));
  bennett_h(&phase);
  if (q & 1)
    y = compute(q);
  if (compute(q) & 1)
    bennett_z(&phase);
  u64 r = bennett_measure(&y);
  return r ^ bennett_measure(&q) ^ bennett_measure(&phase);
}
#else
int printf(const char *, ...);
int main(void) {
  for (unsigned k = 0; k < 8; ++k)
    printf("%llu %u\n", (k & 1) ? compute(k) : 0, (unsigned)(compute(k) & 1));
  return 0;
}
#endif
