/* inplace.c: in-place updates of qubit variables, written as ordinary C. Each update g, and its inverse,
 * is listed once below and compiled twice: as a quantum program (x and y in uniform superposition over all
 * 8-bit values, q in superposition when the update is controlled by it, and x0 an entangled copy of the
 * start value of x), and natively with -DNATIVE, which prints g(x, y, q) for every x, y and q. Before the
 * first measurement every branch must hold x = g(x0, y, q), and nothing else; after the update and its
 * inverse, x = x0. */
#ifndef NATIVE
#include "bennett.h"
#endif

#define ROTL(v, s) (unsigned char)((v) << ((s) & 7) | (v) >> (-(s) & 7))
#define ROTR(v, s) (unsigned char)((v) >> ((s) & 7) | (v) << (-(s) & 7))

/* name, update, inverse. 171 = 1/3 and 205 = 1/5 modulo 256. */
#define UPDATES(X)                                                                                  \
  X(add, x += y, x -= y)                                                                            \
  X(sub, x -= y, x += y)                                                                            \
  X(xor, x ^= y, x ^= y)                                                                            \
  X(not, x = ~x, x = ~x)                                                                            \
  X(negate, x = -x, x = -x)                                                                         \
  X(increment, x++, x--)                                                                            \
  X(times3, x *= 3, x *= 171)                                                                       \
  X(rotate, x = ROTL(x, y), x = ROTR(x, y))                                                         \
  X(rotate3, x = ROTL(x, 3), x = ROTR(x, 3))                                                        \
  X(composed, x = (unsigned char)((x ^ y) * 5 + 7), x = (unsigned char)(((x - 7) * 205) ^ y))      \
  X(subtracted_from, x = (unsigned char)(y - ~(x + y)), x = (unsigned char)(x - 2 * y - 1))        \
  X(controlled_add, if (q) x += y, if (q) x -= y)                                                   \
  X(controlled_composed, if (q) x = (unsigned char)(x * 3 + y), if (q) x = (unsigned char)((x - y) * 171)) \
  X(controlled_either, if (q) x += 5; else x = ROTL(x, y), if (q) x -= 5; else x = ROTR(x, y))

#define CONTROLLED(name) (__builtin_strncmp(#name, "controlled", 10) == 0)

#ifdef NATIVE
#include <stdio.h>
#include <string.h>

#define NATIVE_UPDATE(name, update, inverse)                                                        \
  static unsigned char native_##name(unsigned char x, unsigned char y, _Bool q) {                   \
    (void)y;                                                                                        \
    (void)q;                                                                                        \
    update;                                                                                         \
    return x;                                                                                       \
  }
UPDATES(NATIVE_UPDATE)

/* Prints g(x, y, q) for q, y, x in that order of nesting, one per line. */
int main(int argc, char **argv) {
#define RUN(name, update, inverse)                                                                  \
  if (argc == 2 && strcmp(argv[1], #name) == 0) {                                                   \
    for (int q = 0; q < 2; q++)                                                                     \
      for (int y = 0; y < 256; y++)                                                                 \
        for (int x = 0; x < 256; x++)                                                               \
          printf("%d\n", native_##name((unsigned char)x, (unsigned char)y, (_Bool)q));              \
    return 0;                                                                                       \
  }
  UPDATES(RUN)
  return 1;
}

#else

#define EACH_BIT(gate, v)                                                                           \
  for (int i_ = 0; i_ < 8; i_++)                                                                    \
  gate(bennett_bit(&(v), i_))

/* The registers, in this order: x0, x, y, q. q is put in superposition only for a controlled update, so
 * that the other states stay at 2^16 branches. */
#define START                                                                                       \
  unsigned char x0 = 0, x = 0, y = 0;                                                               \
  _Bool q = 0;                                                                                      \
  EACH_BIT(bennett_h, x);                                                                           \
  EACH_BIT(bennett_h, y);                                                                           \
  if (controlled)                                                                                   \
    bennett_h(&q);                                                                                  \
  x0 ^= x;

#define END                                                                                         \
  unsigned long long r = bennett_measure(&x);                                                       \
  r |= bennett_measure(&x0) << 8;                                                                   \
  r |= bennett_measure(&y) << 16;                                                                   \
  return r | bennett_measure(&q) << 24;

#define QUANTUM_UPDATE(name, update, inverse)                                                       \
  unsigned long long name(void) {                                                                   \
    const int controlled = CONTROLLED(name);                                                        \
    START                                                                                           \
    update;                                                                                         \
    END                                                                                             \
  }                                                                                                 \
  unsigned long long name##_undone(void) {                                                          \
    const int controlled = CONTROLLED(name);                                                        \
    START                                                                                           \
    update;                                                                                         \
    inverse;                                                                                        \
    END                                                                                             \
  }
UPDATES(QUANTUM_UPDATE)

/* A byte swap and a 16-bit multiplication: x0 and x are 16 bits, all of them in superposition. */
unsigned long long swapped(void) {
  unsigned short x0 = 0, x = 0;
  for (int i = 0; i < 16; i++)
    bennett_h(bennett_bit(&x, i));
  x0 ^= x;
  x = (unsigned short)(__builtin_bswap16(x) * 3u + 1);
  unsigned long long r = bennett_measure(&x);
  return r | bennett_measure(&x0) << 16;
}

#endif
