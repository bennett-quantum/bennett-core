/* thesis_kept.c: the dependence is kept, because y is measured. Just before the first measurement the
 * state is (b=0, y=0) and (b=1, y=10), 1/sqrt(2) each; the two measurements agree on every run. */
#include "bennett.h"

static int f(int x) { return x * x + 1; }

int main(void) {
    _Bool b = 0;
    int y = 0;
    bennett_h(&b);
    y = b ? f(3) : 0;
    int measured = (int)bennett_measure(&y);
    return measured == (bennett_measure(&b) ? f(3) : 0);
}
