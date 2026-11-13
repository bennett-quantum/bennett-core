/* coherent.c: each corpus function applied to an 8-bit input in uniform superposition.
 *
 * coherent_<f>(key): eight qubits, each put through a Hadamard; x is assembled from them by ordinary
 * code and XORed with a mask computed from the ordinary argument `key`, so it still ranges over all 256
 * values; out = f(x). Just before the first measurement every branch holds (x, f(x)). Returns the
 * measured x in bits 32..39 and the measured out in bits 0..31. */
#include "bennett.h"

#include "corpus.c"

#define COHERENT(f)                                                                                          \
  unsigned long long coherent_##f(unsigned char key) {                                                       \
    _Bool q[8] = {0};                                                                                        \
    for (int i = 0; i < 8; i++) bennett_h(&q[i]);                                                            \
    unsigned char x = (unsigned char)(key * 37 + 11);                                                        \
    for (int i = 0; i < 8; i++) x ^= (unsigned char)(q[i] << i);                                             \
    int out = f(x);                                                                                          \
    unsigned long long y = bennett_measure(&out);                                                            \
    unsigned long long bits = bennett_measure(q);                                                            \
    unsigned long long measured = 0;                                                                         \
    for (int i = 0; i < 8; i++) measured |= (bits >> (8 * i) & 1) << i;                                     \
    return (measured ^ (unsigned char)(key * 37 + 11)) << 32 | (y & 0xffffffffu);                            \
  }

CORPUS(COHERENT)
