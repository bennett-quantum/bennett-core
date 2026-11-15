/* The default floating-point environment: to nearest, no flags; saving and restoring it works. */
#include <fenv.h>
#include <stdio.h>
int main(void) {
    fenv_t saved;
    int a = fegetround() == FE_TONEAREST, b = fesetround(FE_TONEAREST);
    int c = fegetenv(&saved), d = feholdexcept(&saved), e = fesetenv(&saved), f = fesetenv(FE_DFL_ENV);
    int g = feclearexcept(FE_ALL_EXCEPT);
    printf("%d %d %d %d %d %d %d\n", a, b, c, d, e, f, g);
    return 0;
}
