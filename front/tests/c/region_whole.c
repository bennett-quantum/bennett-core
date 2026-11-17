/* Code without gates whose control flow depends on a qubit, written inline, that writes qubit variables
 * (docs/DESIGN.md, "Code under a qubit"): each body is compiled natively for every value of x and, on the
 * device, run once on x in uniform superposition. clang -O2's own vectoriser is disabled in the inner loop:
 * the front does not follow an accumulation through vector lanes. */
#define INLINE static inline __attribute__((always_inline))

/* Two nested loops whose trip counts depend on x: a and b are only XORed into (their values before are not
 * known to the front), c is counted from its known 0. */
INLINE void nested(unsigned char x, unsigned char *a, unsigned short *b, unsigned short *c) {
    for (unsigned t = x; t != 0; t >>= 1)
#pragma clang loop vectorize(disable)
        for (unsigned j = 0; j <= (t & 3); j++) {
            *a ^= (unsigned char)(j + t);
            *b ^= (unsigned short)((j * t) << 3);
            *c += (unsigned short)(t + j);
        }
}

/* The parity of x, accumulated into y. */
INLINE void parity(unsigned char x, _Bool *y) {
    for (unsigned t = x; t != 0; t >>= 1) *y ^= t & 1;
}

/* A variable with a known value before the code, read and overwritten in a loop that runs x times. */
INLINE void known(unsigned char x, unsigned short *c) {
    if (x > 2)
        for (unsigned j = 0; j < x; j++) *c = (unsigned short)(*c * 3 + j);
}

#ifdef NATIVE
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    unsigned char x = (unsigned char)strtoul(argv[1], 0, 10);
    unsigned char a = 16;
    unsigned short b = 2, c = 0, k = 5;
    _Bool y = 1;
    nested(x, &a, &b, &c);
    parity(x, &y);
    known(x, &k);
    printf("%u %u %u %u %u\n", a, b, c, y, k);
    return 0;
}
#else
#include "bennett.h"

#define superpose(x) (bennett_h(bennett_bit(x, 0)), bennett_h(bennett_bit(x, 1)), bennett_h(bennett_bit(x, 2)))

unsigned long long nested_entry(void) {
    unsigned char x = 0, a = 0;
    unsigned short b = 0, c = 0;
    superpose(&x);
    bennett_x(bennett_bit(&a, 4)); /* a = 16, b = 2: values the front does not know */
    bennett_x(bennett_bit(&b, 1));
    nested(x, &a, &b, &c);
    unsigned long long r = bennett_measure(&x);
    r |= bennett_measure(&a) << 8;
    r |= bennett_measure(&b) << 16;
    return r | bennett_measure(&c) << 32;
}

unsigned long long parity_entry(void) {
    unsigned char x = 0;
    _Bool y = 0;
    superpose(&x);
    bennett_x(&y);
    parity(x, &y);
    unsigned long long r = bennett_measure(&x);
    return r | bennett_measure(&y) << 8;
}

unsigned long long known_entry(void) {
    unsigned char x = 0;
    unsigned short c = 5;
    superpose(&x);
    known(x, &c);
    unsigned long long r = bennett_measure(&x);
    return r | bennett_measure(&c) << 8;
}
#endif
