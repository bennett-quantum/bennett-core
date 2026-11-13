/* kickback.c: compute, act on the result, uncompute. hit = (f(b ? 3 : 0) == 10) is computed into a
 * qubit, a Z gate marks it, and the same expression is XORed in again to uncompute it. The phase is left
 * on b, so the final Hadamard turns b into 1 on every run. Without the uncomputation hit stays entangled
 * with b (kickback_no_uncompute.c). */
#include "bennett.h"

static int f(int x) { return x * x + 1; }

int main(void) {
    _Bool b = 0, hit = 0;
    bennett_h(&b);
    hit ^= f(b ? 3 : 0) == 10; /* compute */
    bennett_z(&hit);
    hit ^= f(b ? 3 : 0) == 10; /* uncompute */
    bennett_h(&b);
    return (int)bennett_measure(&b);
}
