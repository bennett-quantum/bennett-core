/* Another rounding mode is not supported: a named fault, never a success that changes nothing. */
#include <fenv.h>
volatile double one = 1.0, three = 3.0;
int main(void) {
    fesetround(FE_UPWARD);
    return one / three > 0.3333333333333333 ? 0 : 1;
}
