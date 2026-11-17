/* Loops with gates under a condition that depends on a qubit (docs/DESIGN.md, "Code under a qubit"): the
 * loop's trip count n is an ordinary value, so the loop runs in the control program, its gates controlled by
 * the condition. */
#include "bennett.h"

/* The front audit's witness: y is flipped n times where q is 1. */
unsigned long long flips(unsigned n) {
    _Bool q = 0, y = 0;
    bennett_h(&q);
    if (q) for (unsigned i = 0; i < n; i++) bennett_x(&y);
    unsigned long long r = bennett_measure(&q);
    return r | bennett_measure(&y) << 1;
}

/* A computed condition, and two gate sites in the loop, one of them on odd passes only. */
unsigned long long phases(unsigned n) {
    unsigned char x = 0;
    _Bool y = 0;
    bennett_h(bennett_bit(&x, 0));
    bennett_h(bennett_bit(&x, 1));
    if (x * 3 % 5 == 1)
        for (unsigned i = 0; i < n; i++) {
            bennett_h(&y);
            if (i & 1) bennett_z(&y);
        }
    unsigned long long r = bennett_measure(&x);
    return r | bennett_measure(&y) << 2;
}
