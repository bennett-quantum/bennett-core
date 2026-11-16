/* Minimal witnesses. Raw writes are intentional: these are the encodings under test.
 * Native libgcc powi with exponent 1 returns its argument without arithmetic, preserving the bits.
 * glibc roundevenl inspects the encoding in software; for the tiny unnormal below it returns +0.
 * The native results are checked before comparing the independently lowered device program. */
#include <stdint.h>
#include <string.h>
static volatile int exponent = 1;
static volatile long double input;
static void set(unsigned short e, uint64_t m) {
  long double x = 0;
  memcpy(&x, &m, 8);
  memcpy((char *)&x + 8, &e, 2);
  memcpy((void *)&input, &x, sizeof x);
}
int powi_preserves_pseudo(void) {
  set(0, UINT64_C(0x8000000000000000));
  long double y = __builtin_powil(input, exponent);
  unsigned short e;
  memcpy(&e, (char *)&y + 8, 2);
  return e != 0;
}
int powi_preserves_unsupported(void) {
  set(1, 0);
  long double y = __builtin_powil(input, exponent);
  /* Copying the return bits and classifying them is observable without subsequent arithmetic. */
  unsigned short e;
  memcpy(&e, (char *)&y + 8, 2);
  return e != 1;
}
int roundeven_unnormal(void) {
  set(1, 1);
  long double y = __builtin_roundevenl(input);
  /* Native +0 becomes 1 after addition, whereas the runtime's indefinite stays NaN. */
  return __builtin_isnan(y + 1.0L);
}
int pseudo_is_finite(void) {
  set(0, UINT64_C(0x8000000000000000));
  return !__builtin_isfinite(input);
}
int unnormal_is_nan(void) {
  set(1, 1);
  return !__builtin_isnan(input);
}
int round_unnormal(void) {
  set(1, 1);
  return __builtin_isnan(__builtin_roundl(input) + 1.0L);
}
int half_unnormal(void) {
  set(1, 1);
  _Float16 h = input;
  unsigned short bits;
  memcpy(&bits, &h, 2);
  return bits != 0;
}
int bfloat_unnormal(void) {
  set(1, 1);
#ifdef NATIVE
  /* clang 18 cannot select the direct long double -> bfloat operation on x86-64. */
  extern __bf16 __truncxfbf2(long double);
  __bf16 h = __truncxfbf2(input);
#else
  __bf16 h = input;
#endif
  unsigned short bits;
  memcpy(&bits, &h, 2);
  return bits != 0;
}
#ifdef NATIVE
#include <stdio.h>
int main(void) {
  printf("%d %d %d %d %d %d %d %d\n", powi_preserves_pseudo(), powi_preserves_unsupported(), roundeven_unnormal(),
         pseudo_is_finite(), unnormal_is_nan(), round_unnormal(), half_unnormal(), bfloat_unnormal());
  return 0;
}
#endif
