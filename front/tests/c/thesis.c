/* thesis.c: the thesis, exactly as stated. LLVM sees that y ^= (the same value) leaves y = 0 and that y
 * is never used, and deletes the dependence: what runs is h, h, measure. */
#include "bennett.h"

static int f(int x) { return x * x + 1; }

int main(void) {
    _Bool b = 0;
    bennett_h(&b);              /* an ordinary bit becomes a qubit */
    int y = b ? f(3) : 0;       /* ordinary code now depends on it */
    y ^= b ? f(3) : 0;          /* ...and undoes the dependence */
    bennett_h(&b);
    return bennett_measure(&b); /* 0 on every run, only if nothing stayed entangled */
}
