/* More than one operation in an expression, explicit casts, compound
 * assignment, and comparisons. Native clang retains float precision in a
 * short-format expression until an assignment/cast. */
typedef unsigned long long U;
static volatile unsigned short values[][3] = {
    {0x3c00, 0x1000, 0xbc00}, {0x3f80, 0x3b80, 0xbf80},
    {0x3c01, 0x1000, 0xbc00}, {1, 0x3800, 1},
    {0x0400, 0x3800, 0x8001}, {0x7bff, 0x3c01, 0xfbff},
    {0x3f81, 0x3b80, 0xbf80}, {1, 0x3f00, 1},
    {0x0080, 0x3f00, 0x8001}, {0x7f7f, 0x3f81, 0xff7f},
    {0, 0x8000, 0},           {0x8000, 0, 0x8000},
    {0x7c01, 0x3c00, 0},      {0x7f81, 0x3f80, 0},
    {0xfc01, 0x3800, 1},      {0xff81, 0x3f00, 1}};
#define N (sizeof values / sizeof values[0])
#define DEFINE(T, NAME, LIMIT)                                                 \
  static U NAME(unsigned k, unsigned i) {                                      \
    unsigned short v = values[i][0], w = values[i][1], u = values[i][2], bits; \
    T a, b, c, r;                                                              \
    __builtin_memcpy(&a, &v, 2);                                               \
    __builtin_memcpy(&b, &w, 2);                                               \
    __builtin_memcpy(&c, &u, 2);                                               \
    switch (k) {                                                               \
    case 0:                                                                    \
      r = (a + b) + c;                                                         \
      break;                                                                   \
    case 1:                                                                    \
      r = (T)(a + b) + c;                                                      \
      break;                                                                   \
    case 2:                                                                    \
      r = a + b;                                                               \
      r += c;                                                                  \
      break;                                                                   \
    case 3:                                                                    \
      r = a;                                                                   \
      r += b + c;                                                              \
      break;                                                                   \
    case 4:                                                                    \
      r = a * b + c;                                                           \
      break;                                                                   \
    case 5:                                                                    \
      r = (T)(a * b) + c;                                                      \
      break;                                                                   \
    case 6:                                                                    \
      r = (a / b) * c;                                                         \
      break;                                                                   \
    case 7:                                                                    \
      r = (T)(a / b) * c;                                                      \
      break;                                                                   \
    case 8:                                                                    \
      return ((a + b) + c) == a;                                               \
    case 9:                                                                    \
      return (T)(a + b) == (T)(a - c);                                         \
    case 10:                                                                   \
      return (a * b + c) < 0;                                                  \
    case 11:                                                                   \
      return (int)(a + b + c);                                                 \
    case 12:                                                                   \
      r = a;                                                                   \
      r++;                                                                     \
      break;                                                                   \
    case 13:                                                                   \
      r = a;                                                                   \
      ++r;                                                                     \
      break;                                                                   \
    case 14:                                                                   \
      r = a;                                                                   \
      r *= b;                                                                  \
      r += c;                                                                  \
      break;                                                                   \
    case 15:                                                                   \
      r = (a + b) * (a - b);                                                   \
      break;                                                                   \
    case 16:                                                                   \
      r = -(a + b);                                                            \
      break;                                                                   \
    default:                                                                   \
      r = (a > b ? a : b) + c;                                                 \
      break;                                                                   \
    }                                                                          \
    __builtin_memcpy(&bits, &r, 2);                                            \
    return (((bits & 0x7fff) > LIMIT) ? 0x7fff : bits);                        \
  }
DEFINE(_Float16, half, 0x7c00)
DEFINE(__bf16, brain, 0x7f80)
int results(void) { return 2 * 18 * N; }
U result(int i) {
  return i / (18 * N) ? brain(i / N % 18, i % N) : half(i / N % 18, i % N);
}
#ifdef NATIVE
#include <stdio.h>
int main(void) {
  for (int i = 0; i < results(); i++) {
    U r = result(i);
    printf("%016llx%016llx\n", 0ULL, (unsigned long long)r);
  }
}
#endif
