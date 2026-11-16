/* Complex arithmetic with the C library: products and quotients of float and double values through
 * libgcc's helpers (rt/libgcc), musl's cexp and conj, and an Annex G case (a nonzero value over
 * zero is infinite). The printed values are the same with libgcc's helpers and with compiler-rt's. */
#include <complex.h>
#include <stdio.h>

volatile double a = 1, b = 2, c = 3, d = 4;
volatile float fa = 1e-30f, fb = 2, fc = 0, fd = 0;

int main(void) {
  double _Complex x = a + b * I, y = c + d * I;
  float _Complex u = fa + fb * I, v = fc + fd * I, w = u / v;
  double _Complex p = x * y, q = x / y, r = cexp(x) * conj(y);
  printf("%.17g %.17g\n%.17g %.17g\n%.17g %.17g\n%g %g\n", creal(p), cimag(p), creal(q), cimag(q), creal(r),
         cimag(r), crealf(w), cimagf(w));
  return 0;
}
