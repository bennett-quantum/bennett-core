/* float_compare.c: every comparison C can write, on pairs of directed values including NaNs, signed
 * zeros, infinities and subnormals, for double and for float; and the classification builtins.
 * result(i) is a mask of the comparisons that hold for the i-th pair. From optimised clang output the
 * negated comparisons are the unordered fcmp predicates (`!(a < b)` is `fcmp uge`), so every predicate
 * is met. See float_values.c for how the results are checked. */

typedef unsigned long long u64;

#define N 12
static volatile double D[N] = {0.0, -0.0, 1.0, -1.0, 2.0, 0x1p-1074, 0x1p-1022, 0x1.fffffffffffffp1023,
                               __builtin_inf(), -__builtin_inf(), __builtin_nan(""), -__builtin_nan("7")};
static volatile float F[N] = {0.0f, -0.0f, 1.0f, -1.0f, 2.0f, 0x1p-149f, 0x1p-126f, 0x1.fffffep127f,
                              __builtin_inff(), -__builtin_inff(), __builtin_nanf(""), -__builtin_nanf("7")};

#define COMPARISONS(a, b)                                                                                  \
  ((u64)((a) == (b)) | (u64)((a) != (b)) << 1 | (u64)((a) < (b)) << 2 | (u64)((a) <= (b)) << 3 |            \
   (u64)((a) > (b)) << 4 | (u64)((a) >= (b)) << 5 | (u64)!((a) < (b)) << 6 | (u64)!((a) <= (b)) << 7 |      \
   (u64)!((a) > (b)) << 8 | (u64)!((a) >= (b)) << 9 | (u64)__builtin_isunordered(a, b) << 10 |              \
   (u64)!__builtin_isunordered(a, b) << 11 | (u64)__builtin_islessgreater(a, b) << 12 |                     \
   (u64)((a) == (b) || __builtin_isunordered(a, b)) << 13 | (u64)__builtin_isless(a, b) << 14 |             \
   (u64)__builtin_isgreaterequal(a, b) << 15 | (u64)((a) < (b) ? 1 : 0) << 16)

/* The classes: isnan, isinf, isfinite, isnormal, signbit, fpclassify, and __builtin_isfpclass masks. */
#define CLASSES(x)                                                                                         \
  ((u64)__builtin_isnan(x) | (u64)__builtin_isinf(x) << 1 | (u64)__builtin_isfinite(x) << 2 |              \
   (u64)__builtin_isnormal(x) << 3 | (u64)(__builtin_signbit(x) != 0) << 4 |                                 \
   (u64)__builtin_fpclassify(0, 1, 2, 3, 4, x) << 5 | (u64)__builtin_isfpclass(x, 0x003) << 8 |              \
   (u64)__builtin_isfpclass(x, 0x204) << 9 | (u64)__builtin_isfpclass(x, 0x108) << 10 |                     \
   (u64)__builtin_isfpclass(x, 0x090) << 11 | (u64)__builtin_isfpclass(x, 0x060) << 12 |                    \
   (u64)__builtin_isfpclass(x, 0x001) << 13 | (u64)__builtin_isfpclass(x, 0x2AA) << 14)

unsigned results(void) { return 2 * N * N + 2 * N + 2; }

u64 result(unsigned i) {
  if (i < N * N) {
    double a = D[i / N], b = D[i % N];
    return COMPARISONS(a, b);
  }
  i -= N * N;
  if (i < N * N) {
    float a = F[i / N], b = F[i % N];
    return COMPARISONS(a, b);
  }
  i -= N * N;
  if (i < N) {
    double x = D[i];
    return CLASSES(x);
  }
  i -= N;
  if (i < N) {
    float x = F[i];
    return CLASSES(x);
  }
  i -= N;
  /* A signalling NaN is a NaN, and not a quiet one. */
  static volatile unsigned long long snan = 0x7FF0000000000001;
  static volatile unsigned snanf = 0x7F800001;
  double d;
  float f;
  __builtin_memcpy(&d, (const void *)&snan, 8);
  __builtin_memcpy(&f, (const void *)&snanf, 4);
  return i == 0 ? CLASSES(d) : CLASSES(f);
}

#ifdef NATIVE
int printf(const char *, ...);
int main(void) {
  for (unsigned i = 0; i < results(); i++) {
    printf("%llu\n", result(i));
  }
  return 0;
}
#endif
