/* kickback_no_uncompute.c: kickback.c without the uncomputation. hit is still entangled with b when its
 * scope ends, so its release (the partial trace) leaves b in an equal mixture: b measures 0 or 1, each about
 * half the time, where kickback.c gives 1 on every run. The simulator reports the release (weight 1/2). */
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
