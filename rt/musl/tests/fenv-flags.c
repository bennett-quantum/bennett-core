/* Exception flags: 0/0 raises invalid, as natively. */
#include <fenv.h>
volatile double zero = 0.0;
int main(void) {
    feclearexcept(FE_ALL_EXCEPT);
    volatile double q = zero / zero;
    (void)q;
    return fetestexcept(FE_INVALID) ? 0 : 1;
}
