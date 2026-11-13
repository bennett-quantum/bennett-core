/* kickback_no_uncompute.c: kickback.c without the uncomputation. hit is still entangled with b when its
 * scope ends, so releasing it is a dirty discard: a fault on every run, never a wrong answer. */
#include "bennett.h"

static int f(int x) { return x * x + 1; }

int main(void) {
    _Bool b = 0, hit = 0;
    bennett_h(&b);
    hit ^= f(b ? 3 : 0) == 10;
    bennett_z(&hit);
    bennett_h(&b);
    return (int)bennett_measure(&b);
}
