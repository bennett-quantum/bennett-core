/* Integer powers (`__builtin_powi`, libgcc's __powisf2, __powidf2, __powixf2) of the smallest subnormal of each
 * format and of other operands, with exponents that do and do not run the multiplications and the division: the
 * result and both units' six status flags, against the native program (`front/tests/tests/fenv.rs`). The
 * denormal-operand flag, which this checks most, is outside the guarantee: compared as best effort. */
#include <fenv.h>
#include <stdio.h>
#include <string.h>

#pragma STDC FENV_ACCESS ON

static volatile float floats[] = {0x1p-149f, 0x1.8p-127f, 1.5f, 0x1p100f, 0.0f};
static volatile double doubles[] = {0x1p-1074, 0x1.8p-1023, 1.5, 0x1p600, 0.0};
static volatile long double longs[] = {0x1p-16445L, 0x1.8p-16383L, 1.5L, 0x1p9000L, 0.0L};
static volatile int exponents[] = {0, 1, 2, 3, -1, -2, 5};

static void show(const char *what, unsigned i, unsigned j, const void *bits, size_t n) {
  fenv_t e;
  fegetenv(&e);
  printf("%s %u %u ", what, i, j);
  for (size_t k = n; k-- > 0;) printf("%02x", ((const unsigned char *)bits)[k]);
  printf(" sse %02x x87 %02x\n", e.__mxcsr & 0x3f, e.__status_word & 0x3f);
}

int main(void) {
  for (unsigned i = 0; i < 5; ++i)
    for (unsigned j = 0; j < 7; ++j) {
      fesetenv(FE_DFL_ENV);
      volatile float f = __builtin_powif(floats[i], exponents[j]);
      show("f", i, j, (const void *)&f, 4);
      fesetenv(FE_DFL_ENV);
      volatile double d = __builtin_powi(doubles[i], exponents[j]);
      show("d", i, j, (const void *)&d, 8);
      fesetenv(FE_DFL_ENV);
      volatile long double l = __builtin_powil(longs[i], exponents[j]);
      show("l", i, j, (const void *)&l, 10);
    }
  fesetenv(FE_DFL_ENV);
  return 0;
}
