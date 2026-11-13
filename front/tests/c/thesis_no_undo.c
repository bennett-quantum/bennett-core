/* thesis_no_undo.c: the thesis without the undoing line. y is never used, so by C's semantics it does
 * not exist: no dependence is left behind, and the measurement is still 0 on every run. */
#include "bennett.h"

static int f(int x) { return x * x + 1; }

int main(void) {
    _Bool b = 0;
    bennett_h(&b);
    int y = b ? f(3) : 0;
    (void)y;
    bennett_h(&b);
    return bennett_measure(&b);
}
