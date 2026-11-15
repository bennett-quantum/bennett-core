/* Both signednesses, every floating format, and widths near legalisation
 * boundaries. Out-of-range conversions are excluded: native narrow _BitInt
 * casts can differ between -O0 and -O2. */
typedef unsigned __int128 U;
typedef unsigned long long u64;
static U fb(float x) {
  unsigned b;
  __builtin_memcpy(&b, &x, 4);
  return b;
}
static U db(double x) {
  u64 b;
  __builtin_memcpy(&b, &x, 8);
  return b;
}
static U lb(long double x) {
  u64 b;
  unsigned short e;
  __builtin_memcpy(&b, &x, 8);
  __builtin_memcpy(&e, (char *)&x + 8, 2);
  return (U)e << 64 | b;
}
static U hb(_Float16 x) {
  unsigned short b;
  __builtin_memcpy(&b, &x, 2);
  return b;
}
static U bb(__bf16 x) {
  unsigned short b;
  __builtin_memcpy(&b, &x, 2);
  return b;
}
static volatile long double f[] = {0.L,
                                   -0.L,
                                   1.L,
                                   -1.L,
                                   0.5L,
                                   -0.5L,
                                   127.5L,
                                   255.5L,
                                   32768.L,
                                   65504.L,
                                   0x1p31L - 0.5L,
                                   0x1p31L,
                                   0x1p32L - 1.L,
                                   0x1p32L,
                                   0x1p63L - 0.5L,
                                   0x1p63L,
                                   0x1p63L + 1.L,
                                   0x1p64L - 1.L,
                                   0x1p64L,
                                   0x1p64L + 2.L,
                                   0x1p65L - 2.L,
                                   0x1p65L,
                                   0x1p100L - 0x1p36L,
                                   0x1p127L - 0x1p63L,
                                   0x1p127L,
                                   0x1p127L + 0x1p64L,
                                   0x1p128L - 0x1p64L,
                                   0x1p128L,
                                   -0x1p63L,
                                   -0x1p64L,
                                   -0x1p127L,
                                   1.L / 0.L,
                                   -1.L / 0.L,
                                   0.L / 0.L};
#define NF (sizeof f / sizeof f[0])
static volatile U integers[] = {0,
                                1,
                                127,
                                255,
                                32767,
                                65535,
                                0xffffff,
                                0x1000001,
                                0x7fffffff,
                                0x80000001,
                                (U)1 << 63,
                                ((U)1 << 64) - 1,
                                (U)1 << 64,
                                ((U)1 << 64) + 1,
                                ((U)1 << 100) + 1,
                                ((U)1 << 127) - 1,
                                (U)1 << 127,
                                ((U)1 << 127) + 1,
                                (U)-1};
#define NI (sizeof integers / sizeof integers[0])
#define PER (10 * (NF + NI))
#define SIGNED(N, X)                                                           \
  ({                                                                           \
    __typeof__(X) y = (X);                                                     \
    long double z = (long double)y;                                            \
    (z >= -(long double)((U)1 << (N - 1)) &&                                   \
     z < (long double)((U)1 << (N - 1)))                                       \
        ? (U)(_BitInt(N))y                                                     \
        : (U)0;                                                                \
  })
#define UNSIGNED(N, X)                                                         \
  ({                                                                           \
    __typeof__(X) y = (X);                                                     \
    long double z = (long double)y;                                            \
    (z >= 0 && z < 2.L * (long double)((U)1 << (N - 1)))                       \
        ? (U)(unsigned _BitInt(N))y                                            \
        : (U)0;                                                                \
  })
#define WIDTH(N)                                                               \
  case N: {                                                                    \
    if (i < 10 * NF) {                                                         \
      long double x = f[i / 10];                                               \
      switch (i % 10) {                                                        \
      case 0:                                                                  \
        return SIGNED(N, (float)x);                                            \
      case 1:                                                                  \
        return UNSIGNED(N, (float)x);                                          \
      case 2:                                                                  \
        return SIGNED(N, (double)x);                                           \
      case 3:                                                                  \
        return UNSIGNED(N, (double)x);                                         \
      case 4:                                                                  \
        return SIGNED(N, x);                                                   \
      case 5:                                                                  \
        return UNSIGNED(N, x);                                                 \
      case 6:                                                                  \
        return SIGNED(N, (_Float16)x);                                         \
      case 7:                                                                  \
        return UNSIGNED(N, (_Float16)x);                                       \
      case 8:                                                                  \
        return SIGNED(N, (__bf16)(float)x);                                    \
      default:                                                                 \
        return UNSIGNED(N, (__bf16)(float)x);                                  \
      }                                                                        \
    }                                                                          \
    U x = integers[(i - 10 * NF) / 10];                                        \
    _BitInt(N) s = (_BitInt(N))x;                                              \
    unsigned _BitInt(N) u = x;                                                 \
    switch (i % 10) {                                                          \
    case 0:                                                                    \
      return fb((float)s);                                                     \
    case 1:                                                                    \
      return fb((float)u);                                                     \
    case 2:                                                                    \
      return db((double)s);                                                    \
    case 3:                                                                    \
      return db((double)u);                                                    \
    case 4:                                                                    \
      return lb((long double)s);                                               \
    case 5:                                                                    \
      return lb((long double)u);                                               \
    case 6:                                                                    \
      return hb((_Float16)s);                                                  \
    case 7:                                                                    \
      return hb((_Float16)u);                                                  \
    case 8:                                                                    \
      return bb((__bf16)s);                                                    \
    default:                                                                   \
      return bb((__bf16)u);                                                    \
    }                                                                          \
  }
#ifndef TEST_WIDTH
#define TEST_WIDTH 128
#endif
int results(void) { return 2 * PER; }
static U value(int j) {
  int i = j % PER;
  switch (TEST_WIDTH) {
    WIDTH(TEST_WIDTH)
  default:
    return 0;
  }
}
u64 result(int i) {
  U v = value(i / 2);
  return i % 2 ? (u64)v : (u64)(v >> 64);
}
#ifdef NATIVE
#include <stdio.h>
int main(void) {
  for (int i = 0; i < results(); i++) {
    U r = result(i);
    printf("%016llx%016llx\n", (u64)(r >> 64), (u64)r);
  }
}
#endif
