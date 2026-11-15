/* Correlated rounding witnesses, rather than independent random bit patterns:
 * exponents on both sides of under/overflow and 64/128/192/256-bit alignment
 * boundaries; neighbouring rounded products and ties. The runtime bitcode is
 * compiled natively with rt_ symbol prefixes, as in softfloat.rs. */
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

long double rt_fmal(long double, long double, long double);
static unsigned long long checked;
static unsigned failures;
static void check(long double a, long double b, long double c) {
  volatile long double x = a, y = b, z = c;
  long double native = fmal(x, y, z), runtime = rt_fmal(x, y, z);
  ++checked;
  if (memcmp(&native, &runtime, 10) && !(isnan(native) && isnan(runtime))) {
    if (failures++ < 8)
      printf("a=%La b=%La c=%La native=%La runtime=%La\n", a, b, c, native,
             runtime);
  }
}
typedef unsigned __int128 U;
typedef __int128 S;
S rt___fixsfti(float);
U rt___fixunssfti(float);
S rt___fixdfti(double);
U rt___fixunsdfti(double);
S rt___fixxfti(long double);
U rt___fixunsxfti(long double);
float rt___floattisf(S);
float rt___floatuntisf(U);
double rt___floattidf(S);
double rt___floatuntidf(U);
long double rt___floattixf(S);
long double rt___floatuntixf(U);
static unsigned long long int_checked;
#define CHECK_INT(A, B)                                                        \
  do {                                                                         \
    U a_ = (A), b_ = (B);                                                      \
    ++int_checked;                                                             \
    if (a_ != b_) {                                                            \
      if (failures++ < 8)                                                      \
        printf("integer conversion mismatch at line %d\n", __LINE__);          \
    }                                                                          \
  } while (0)
#define TO(T, SUFFIX, X)                                                       \
  do {                                                                         \
    volatile T x_ = (X);                                                       \
    CHECK_INT((U)(S)x_, (U)rt___fix##SUFFIX##ti(x_));                          \
    CHECK_INT((U)x_, rt___fixuns##SUFFIX##ti(x_));                             \
  } while (0)
#define FROM(T, SUFFIX, X, SIGN)                                               \
  do {                                                                         \
    T n_ = (T)(SIGN)(X), r_ = rt___float##SUFFIX((SIGN)(X));                   \
    ++int_checked;                                                             \
    if (memcmp(&n_, &r_, sizeof(T) == 16 ? 10 : sizeof(T))) {                  \
      if (failures++ < 8)                                                      \
        printf("from integer mismatch at line %d\n", __LINE__);                \
    }                                                                          \
  } while (0)
static void integers(void) {
  int powers[] = {0, 1, 7, 8, 15, 16, 31, 32, 63, 64, 65, 100, 126, 127, 128};
  for (unsigned i = 0; i < sizeof powers / sizeof *powers; i++)
    for (int sign = -1; sign <= 1; sign += 2) {
      volatile long double base = sign * ldexpl(1.L, powers[i]);
      for (int delta = -8; delta <= 8; delta++) {
        long double x = base;
        double d = base;
        float f = base;
        for (int k = 0; k < abs(delta); k++) {
          x = nextafterl(x, delta < 0 ? -INFINITY : INFINITY);
          d = nextafter(d, delta < 0 ? -INFINITY : INFINITY);
          f = nextafterf(f, delta < 0 ? -INFINITY : INFINITY);
        }
        TO(long double, xf, x);
        TO(double, df, d);
        TO(float, sf, f);
      }
    }
  long double special[] = {0.L,       -0.L,           INFINITY,
                           -INFINITY, nanl("0x1234"), -nanl("0x4567")};
  for (unsigned i = 0; i < sizeof special / sizeof *special; i++) {
    TO(long double, xf, special[i]);
    TO(double, df, special[i]);
    TO(float, sf, special[i]);
  }
  for (int p = 24; p <= 64; p += (p == 24 ? 29 : 11))
    for (int k = p; k < 128; k++)
      for (int offset = -2; offset <= 2; offset++)
        for (int negative = 0; negative < 2; negative++) {
          U u = ((U)1 << k) + ((U)1 << (k - p)) + offset;
          if (negative)
            u = -u;
          FROM(float, tisf, u, S);
          FROM(float, untisf, u, U);
          FROM(double, tidf, u, S);
          FROM(double, untidf, u, U);
          FROM(long double, tixf, u, S);
          FROM(long double, untixf, u, U);
        }
  printf("directed integer conversions: %llu checked\n", int_checked);
}
int main(void) {
  static const int exponents[] = {
      -16445, -16444, -16400, -16383, -16382, -16381, -8200, -8192,
      -65,    -64,    -63,    -1,     0,      1,      63,    64,
      65,     8191,   8192,   16320,  16381,  16382,  16383};
  static const int gaps[] = {0,   1,   2,   31,  32,   62,   63,  64,
                             65,  66,  125, 126, 127,  128,  129, 130,
                             189, 190, 191, 192, 193,  194,  253, 254,
                             255, 256, 257, 258, 1000, 16380};
  static const long double m[] = {1.L,  1.L + 0x1p-63L,  1.5L - 0x1p-63L,
                                  1.5L, 1.5L + 0x1p-63L, 2.L - 0x1p-63L};
  for (unsigned e = 0; e < sizeof exponents / sizeof *exponents; e++)
    for (unsigned j = 0; j < 6; j++)
      for (unsigned k = 0; k < 6; k++) {
        long double a = ldexpl(m[j], exponents[e]), b = m[k];
        volatile long double product = a * b;
        long double c = -product;
        if (isfinite(c))
          for (int n = -2; n <= 2; n++) {
            long double z = c;
            for (int q = 0; q < abs(n); q++)
              z = nextafterl(z, n < 0 ? -INFINITY : INFINITY);
            check(a, b, z);
            check(-a, b, -z);
            check(a, -b, -z);
          }
        for (unsigned g = 0; g < sizeof gaps / sizeof *gaps; g++) {
          long double z = ldexpl(m[(j + k) % 6], exponents[e] - gaps[g]);
          check(a, b, z);
          check(a, b, -z);
          check(-a, b, z);
          check(-a, b, -z);
        }
      }
  /* Exactly at, immediately above, and immediately below a result midpoint. The
   * product's low bits and the addend together decide the sticky bit:
   * especially delicate when subtraction cancels. */
  for (int e = -16440; e <= 16380; e += 61)
    for (int sign = -1; sign <= 1; sign += 2) {
      long double a = ldexpl(1.L + 0x1p-63L, e), b = 1.L - 0x1p-63L;
      check(a, b, -ldexpl(1.L, e));
      for (int n = -2; n <= 2; n++) {
        long double c = ldexpl(1.L, e - 64) + ldexpl((long double)n, e - 127);
        check(a, 1.L, sign * c);
        check(a, b, sign * c);
      }
    }
  integers();
  printf("correlated fmal: %llu checked, %u mismatches\n", checked, failures);
  return failures != 0;
}
