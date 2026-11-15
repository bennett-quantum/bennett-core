/* Exception flags are not readable on the device: testing one is a named fault. */
#include <fenv.h>
volatile double zero = 0.0;
int main(void) {
    feclearexcept(FE_ALL_EXCEPT);
    volatile double q = zero / zero;
    (void)q;
    return fetestexcept(FE_INVALID) ? 0 : 1;
}
