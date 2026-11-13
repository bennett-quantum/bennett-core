/* grover.c: Grover search written in C over ordinary, unmodified C predicates on 8-bit values. Each
 * predicate knows nothing about qubits. The search puts x in uniform superposition, marks it with the
 * predicate, reflects about the uniform superposition, repeats, and measures. With M solutions among
 * N = 256 values and sin(theta) = sqrt(M / N), k iterations find a solution with probability
 * sin^2((2k + 1) theta). With -DNATIVE it prints the predicates' values for every x instead. */

/* One solution: an 8-bit hash that is a bijection. */
int one_solution(unsigned char x) {
  unsigned char h = (unsigned char)(x * 37 + 11);
  h ^= h >> 3;
  return h == 0x5a;
}

/* Several: the decimal digits add up to 12. */
int digit_sum_12(unsigned char x) {
  int sum = 0;
  for (unsigned v = x; v != 0; v /= 10)
    sum += v % 10;
  return sum == 12;
}

#ifdef NATIVE
#include <stdio.h>

int main(void) {
  for (int x = 0; x < 256; x++)
    printf("%d %d\n", one_solution((unsigned char)x), digit_sum_12((unsigned char)x));
  return 0;
}

#else
#include "bennett.h"

#define EACH_BIT(gate, v)                                                                           \
  for (int i_ = 0; i_ < 8; i_++)                                                                    \
  gate(bennett_bit(&(v), i_))

/* `phase` is held at 1 while the search runs, so that a Z on it is a phase of -1 wherever its condition
 * holds; it is 0 again before x is measured. */
#define GROVER(name, predicate)                                                                     \
  unsigned name(unsigned iterations) {                                                              \
    unsigned char x = 0;                                                                            \
    _Bool phase = 0;                                                                                \
    bennett_x(&phase);                                                                              \
    EACH_BIT(bennett_h, x);                                                                         \
    for (unsigned k = 0; k < iterations; k++) {                                                     \
      if (predicate(x)) /* mark the solutions */                                                    \
        bennett_z(&phase);                                                                          \
      EACH_BIT(bennett_h, x); /* reflect about the uniform superposition */                         \
      if (x != 0)                                                                                   \
        bennett_z(&phase);                                                                          \
      EACH_BIT(bennett_h, x);                                                                       \
    }                                                                                               \
    bennett_x(&phase);                                                                              \
    return (unsigned)bennett_measure(&x);                                                           \
  }

GROVER(search_one_solution, one_solution)
GROVER(search_digit_sum_12, digit_sum_12)

#endif
