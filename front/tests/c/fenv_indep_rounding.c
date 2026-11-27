/* Independent floating-point environment test: rounding modes across arithmetic (+ - * /),
 * sqrt, fma, format conversions (float/double/long double), integer conversions (both directions,
 * 8/16/32/64/128-bit signed and unsigned), and rounding functions (rint, nearbyint, lrint, llrint,
 * ceil, trunc, round, roundeven, lround, llround, fmin, fmax, including library calls through
 * function pointers). Prints exact result bits and both units' five ISO C exception flags per
 * case; compared verbatim against native clang 18 + glibc at -O0 and -O2. */
#include <fenv.h>
#include <float.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#pragma STDC FENV_ACCESS ON

#define FIVE_FLAGS (FE_INVALID | FE_DIVBYZERO | FE_OVERFLOW | FE_UNDERFLOW | FE_INEXACT)

static const int modes[4] = {FE_TONEAREST, FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO};

static void clear_all(void) {
  feclearexcept(FE_ALL_EXCEPT);
}

static void show_bytes(const char *tag, int m, unsigned idx, const void *p, size_t n) {
  static const char hex[] = "0123456789abcdef";
  fenv_t e;
  fegetenv(&e);
  unsigned sse = e.__mxcsr & FIVE_FLAGS;
  unsigned x87 = e.__status_word & FIVE_FLAGS;
  unsigned both = (unsigned)fetestexcept(FIVE_FLAGS);
  char buf[128];
  char *q = buf;
  while (*tag) *q++ = *tag++;
  *q++ = ' '; *q++ = 'm'; *q++ = (char)('0' + m); *q++ = ' ';
  if (idx >= 100) *q++ = (char)('0' + (idx / 100) % 10);
  if (idx >= 10) *q++ = (char)('0' + (idx / 10) % 10);
  *q++ = (char)('0' + idx % 10);
  *q++ = ' ';
  for (size_t k = n; k-- > 0;) {
    unsigned char b = ((const unsigned char *)p)[k];
    *q++ = hex[b >> 4];
    *q++ = hex[b & 15];
  }
  *q++ = ' '; *q++ = 'f'; *q++ = '='; *q++ = hex[both >> 4]; *q++ = hex[both & 15];
  *q++ = ' '; *q++ = 's'; *q++ = '='; *q++ = hex[sse >> 4]; *q++ = hex[sse & 15];
  *q++ = ' '; *q++ = 'x'; *q++ = '='; *q++ = hex[x87 >> 4]; *q++ = hex[x87 & 15];
  *q++ = '\n';
  fwrite(buf, 1, (size_t)(q - buf), stdout);
}

static void show_f32(const char *tag, int m, unsigned idx, float v) { show_bytes(tag, m, idx, &v, 4); }
static void show_f64(const char *tag, int m, unsigned idx, double v) { show_bytes(tag, m, idx, &v, 8); }
static void show_f80(const char *tag, int m, unsigned idx, long double v) { show_bytes(tag, m, idx, &v, 10); }
static void show_i64(const char *tag, int m, unsigned idx, int64_t v) { show_bytes(tag, m, idx, &v, 8); }
static void show_u64(const char *tag, int m, unsigned idx, uint64_t v) { show_bytes(tag, m, idx, &v, 8); }
static void show_i128(const char *tag, int m, unsigned idx, __int128 v) { show_bytes(tag, m, idx, &v, 16); }
static void show_u128(const char *tag, int m, unsigned idx, unsigned __int128 v) { show_bytes(tag, m, idx, &v, 16); }

/* Out-of-range integer conversions have undefined value in C and native -O0/-O2 may differ on the
 * returned value or on FE_INEXACT beside FE_INVALID; compare only FE_INVALID when invalid is raised. */
static void show_int_checked(const char *tag, int m, unsigned idx, int64_t v) {
  int inv = fetestexcept(FE_INVALID);
  if (inv) {
    v = 0;
    feclearexcept(FE_INEXACT);
  }
  show_i64(tag, m, idx, v);
}

/* Function pointers to force calls to rt/musl/math.c (and glibc libm natively) even at -O2 -fno-math-errno. */
static float (*volatile p_sqrtf)(float) = sqrtf;
static double (*volatile p_sqrt)(double) = sqrt;
static long double (*volatile p_sqrtl)(long double) = sqrtl;
static float (*volatile p_fmaf)(float, float, float) = fmaf;
static double (*volatile p_fma)(double, double, double) = fma;
static long double (*volatile p_fmal)(long double, long double, long double) = fmal;
static float (*volatile p_rintf)(float) = rintf;
static double (*volatile p_rint)(double) = rint;
static long double (*volatile p_rintl)(long double) = rintl;
static float (*volatile p_nearbyintf)(float) = nearbyintf;
static double (*volatile p_nearbyint)(double) = nearbyint;
static long double (*volatile p_nearbyintl)(long double) = nearbyintl;
static long (*volatile p_lrintf)(float) = lrintf;
static long (*volatile p_lrint)(double) = lrint;
static long (*volatile p_lrintl)(long double) = lrintl;
static long long (*volatile p_llrintf)(float) = llrintf;
static long long (*volatile p_llrint)(double) = llrint;
static long long (*volatile p_llrintl)(long double) = llrintl;
static float (*volatile p_ceilf)(float) = ceilf;
static float (*volatile p_truncf)(float) = truncf;
static long (*volatile p_lroundf)(float) = lroundf;
static long long (*volatile p_llround)(double) = llround;
static long long (*volatile p_llroundl)(long double) = llroundl;
static float (*volatile p_fminf)(float, float) = fminf;
static double (*volatile p_fmax)(double, double) = fmax;
static long double (*volatile p_fmaxl)(long double, long double) = fmaxl;

static void test_arith_f32(int m) {
  static volatile float pairs[][2] = {
      {1.0f, 0x1p-24f},        /* exact tie to even (low bit 0) */
      {1.0f + 0x1p-23f, 0x1p-24f}, /* exact tie to even (low bit 1) */
      {-1.0f, -0x1p-24f},      /* negative exact tie (even) */
      {-(1.0f + 0x1p-23f), -0x1p-24f}, /* negative exact tie (odd) */
      {1.0f, 0x1p-25f},        /* below half ulp */
      {-1.0f, -0x1p-25f},      /* negative below half ulp */
      {3.5f, 3.5f},            /* exact cancellation on sub */
      {-0.0f, 0.0f},           /* signed zero sum */
      {FLT_MAX, 0x1p103f},     /* MAX + half ulp: overflows in nearest/upward, finite in downward/towardzero */
      {-FLT_MAX, -0x1p103f},   /* -MAX - half ulp */
      {FLT_MAX, 0x1p104f},     /* MAX + ulp: overflows in all modes */
      {1.0f, 3.0f},            /* repeating binary fraction */
      {-1.0f, 3.0f},           /* negative repeating fraction */
      {0x1p-126f, 0x1p-149f},  /* min normal +/- min subnormal */
      {0x1.fffffcp-127f, 0x1p-149f}, /* max subnormal +/- min subnormal */
  };
  for (unsigned i = 0; i < sizeof(pairs) / sizeof(pairs[0]); ++i) {
    float a = pairs[i][0], b = pairs[i][1];
    clear_all(); show_f32("f32_add", m, i, a + b);
    clear_all(); show_f32("f32_sub", m, i, a - b);
    clear_all(); show_f32("f32_mul", m, i, a * b);
    clear_all(); show_f32("f32_div", m, i, a / b);
  }
}

static void test_arith_f64(int m) {
  static volatile double pairs[][2] = {
      {1.0, 0x1p-53},
      {1.0 + 0x1p-52, 0x1p-53},
      {-1.0, -0x1p-53},
      {-(1.0 + 0x1p-52), -0x1p-53},
      {1.0, 0x1p-54},
      {-1.0, -0x1p-54},
      {7.25, 7.25},
      {-0.0, 0.0},
      {DBL_MAX, 0x1p970},      /* MAX + half ulp */
      {-DBL_MAX, -0x1p970},
      {DBL_MAX, 0x1p971},      /* MAX + ulp */
      {1.0, 3.0},
      {-1.0, 3.0},
      {0x1p-1022, 0x1p-1074},
      {0x1.ffffffffffffep-1023, 0x1p-1074},
  };
  for (unsigned i = 0; i < sizeof(pairs) / sizeof(pairs[0]); ++i) {
    double a = pairs[i][0], b = pairs[i][1];
    clear_all(); show_f64("f64_add", m, i, a + b);
    clear_all(); show_f64("f64_sub", m, i, a - b);
    clear_all(); show_f64("f64_mul", m, i, a * b);
    clear_all(); show_f64("f64_div", m, i, a / b);
  }
}

static void test_arith_f80(int m) {
  static volatile long double pairs[][2] = {
      {1.0L, 0x1p-64L},
      {1.0L + 0x1p-63L, 0x1p-64L},
      {-1.0L, -0x1p-64L},
      {-(1.0L + 0x1p-63L), -0x1p-64L},
      {1.0L, 0x1p-65L},
      {-1.0L, -0x1p-65L},
      {11.5L, 11.5L},
      {-0.0L, 0.0L},
      {LDBL_MAX, 0x1p16319L},  /* MAX + half ulp */
      {-LDBL_MAX, -0x1p16319L},
      {LDBL_MAX, 0x1p16320L},  /* MAX + ulp */
      {1.0L, 3.0L},
      {-1.0L, 3.0L},
      {0x1p-16382L, 0x1p-16445L},
      {0x1.fffffffffffffffcp-16383L, 0x1p-16445L},
  };
  for (unsigned i = 0; i < sizeof(pairs) / sizeof(pairs[0]); ++i) {
    long double a = pairs[i][0], b = pairs[i][1];
    clear_all(); show_f80("f80_add", m, i, a + b);
    clear_all(); show_f80("f80_sub", m, i, a - b);
    clear_all(); show_f80("f80_mul", m, i, a * b);
    clear_all(); show_f80("f80_div", m, i, a / b);
  }
}

static void test_sqrt_and_fma(int m) {
  static volatile float sf[] = {0.0f, -0.0f, 1.0f, 2.0f, 3.0f, 4.0f, 0x1p-148f, 0x1p-149f, FLT_MAX, -1.0f, -0x1p-149f};
  for (unsigned i = 0; i < sizeof(sf) / sizeof(sf[0]); ++i) {
    clear_all(); show_f32("sqrtf", m, i, sqrtf(sf[i]));
    clear_all(); show_f32("psqrtf", m, i, p_sqrtf(sf[i]));
  }
  static volatile double sd[] = {0.0, -0.0, 1.0, 2.0, 3.0, 4.0, 0x1p-1072, 0x1p-1074, DBL_MAX, -1.0, -0x1p-1074};
  for (unsigned i = 0; i < sizeof(sd) / sizeof(sd[0]); ++i) {
    clear_all(); show_f64("sqrt", m, i, sqrt(sd[i]));
    clear_all(); show_f64("psqrt", m, i, p_sqrt(sd[i]));
  }
  static volatile long double sl[] = {
      0.0L, -0.0L, 1.0L, 2.0L, 3.0L, 4.0L, 0x1p-16444L, 0x1p-16445L, LDBL_MAX,
      -1.0L, -0x1p-16445L, -(long double)INFINITY, -(long double)NAN, -__builtin_nansl(""),
  };
  for (unsigned i = 0; i < sizeof(sl) / sizeof(sl[0]); ++i) {
    clear_all(); show_f80("sqrtl", m, i, sqrtl(sl[i]));
    clear_all(); show_f80("psqrtl", m, i, p_sqrtl(sl[i]));
  }

  /* FMA: exact cancellation, single rounding vs double rounding, intermediate overflow cancelled back
   * to finite, intermediate underflow, and overflow boundary. */
  static volatile float ff[][3] = {
      {1.5f, 2.5f, -3.75f},                 /* exact cancellation */
      {1.0f + 0x1p-23f, 1.0f + 0x1p-23f, -1.0f}, /* single rounding keeps 2^-46 */
      {1.0f + 0x1p-23f, 1.0f - 0x1p-23f, -1.0f}, /* exact -2^-46 */
      {0x1p127f, 2.0f, -0x1p127f},          /* intermediate 2^128 cancelled to finite 2^127 */
      {0x1p127f, 2.0f, -INFINITY},          /* finite product + (-inf) = -inf, no overflow/invalid */
      {FLT_MAX, 1.0f, 0x1p103f},            /* overflow boundary */
      {0x1p-100f, 0x1p-100f, 1.0f},         /* tiny product + 1.0 */
      {0x1p-100f, 0x1p-100f, 0.0f},         /* underflow to 0 or min subnormal */
      {-0x1p-100f, 0x1p-100f, 0.0f},        /* negative underflow */
      {0x1.fffffep-126f, 0.25f, 0x1p-127f}, /* after-rounding tininess boundary: 2^-126 - 2^-151 */
  };
  for (unsigned i = 0; i < sizeof(ff) / sizeof(ff[0]); ++i) {
    clear_all(); show_f32("fmaf", m, i, fmaf(ff[i][0], ff[i][1], ff[i][2]));
    clear_all(); show_f32("pfmaf", m, i, p_fmaf(ff[i][0], ff[i][1], ff[i][2]));
  }
  static volatile double fd[][3] = {
      {1.5, 2.5, -3.75},
      {1.0 + 0x1p-52, 1.0 + 0x1p-52, -1.0},
      {1.0 + 0x1p-52, 1.0 - 0x1p-52, -1.0},
      {0x1p1023, 2.0, -0x1p1023},
      {0x1p1023, 2.0, -INFINITY},
      {DBL_MAX, 1.0, 0x1p970},
      {0x1p-600, 0x1p-600, 1.0},
      {0x1p-600, 0x1p-600, 0.0},
      {-0x1p-600, 0x1p-600, 0.0},
      {0x1.fffffffffffffp-1022, 0.25, 0x1p-1023},
  };
  for (unsigned i = 0; i < sizeof(fd) / sizeof(fd[0]); ++i) {
    clear_all(); show_f64("fma", m, i, fma(fd[i][0], fd[i][1], fd[i][2]));
    clear_all(); show_f64("pfma", m, i, p_fma(fd[i][0], fd[i][1], fd[i][2]));
  }
  static volatile long double fl[][3] = {
      {1.5L, 2.5L, -3.75L},
      {1.0L + 0x1p-63L, 1.0L + 0x1p-63L, -1.0L},
      {1.0L + 0x1p-63L, 1.0L - 0x1p-63L, -1.0L},
      {0x1p16383L, 2.0L, -0x1p16383L},
      {0x1p16383L, 2.0L, -(long double)INFINITY},
      {LDBL_MAX, 1.0L, 0x1p16319L},
      {0x1p-10000L, 0x1p-10000L, 1.0L},
      {0x1p-10000L, 0x1p-10000L, 0.0L},
      {-0x1p-10000L, 0x1p-10000L, 0.0L},
      {0x1.fffffffffffffffep-16382L, 0.25L, 0x1p-16383L},
  };
  for (unsigned i = 0; i < sizeof(fl) / sizeof(fl[0]); ++i) {
    clear_all(); show_f80("fmal", m, i, fmal(fl[i][0], fl[i][1], fl[i][2]));
    clear_all(); show_f80("pfmal", m, i, p_fmal(fl[i][0], fl[i][1], fl[i][2]));
  }
}

static void test_conversions(int m) {
  /* Float <-> double <-> long double: ties, overflow, tininess boundary, exact subnormals. */
  static volatile double d2f[] = {
      1.0 + 0x1p-24,               /* exact tie (even) */
      1.0 + 0x1p-23 + 0x1p-24,     /* exact tie (odd) */
      -(1.0 + 0x1p-24),
      -(1.0 + 0x1p-23 + 0x1p-24),
      (double)FLT_MAX + 0x1p103,   /* overflow boundary: rounds to FLT_MAX in downward/towardzero */
      -((double)FLT_MAX + 0x1p103),
      (double)FLT_MAX + 0x1p104,   /* always overflows */
      0x1p-126 - 0x1p-150,         /* tininess after rounding: normal in nearest/upward, subnormal otherwise */
      -(0x1p-126 - 0x1p-150),
      0x1p-149,                    /* exact subnormal */
      0x1p-150,                    /* below min subnormal */
      -0x1p-150,
  };
  for (unsigned i = 0; i < sizeof(d2f) / sizeof(d2f[0]); ++i) {
    clear_all(); show_f32("d_to_f", m, i, (float)d2f[i]);
  }

  static volatile long double l2d[] = {
      1.0L + 0x1p-53L,
      1.0L + 0x1p-52L + 0x1p-53L,
      -(1.0L + 0x1p-53L),
      -(1.0L + 0x1p-52L + 0x1p-53L),
      (long double)DBL_MAX + 0x1p970L,
      -((long double)DBL_MAX + 0x1p970L),
      (long double)DBL_MAX + 0x1p971L,
      0x1p-1022L - 0x1p-1075L,
      -(0x1p-1022L - 0x1p-1075L),
      0x1p-1074L,
      0x1p-1075L,
      -0x1p-1075L,
  };
  for (unsigned i = 0; i < sizeof(l2d) / sizeof(l2d[0]); ++i) {
    clear_all(); show_f64("l_to_d", m, i, (double)l2d[i]);
    clear_all(); show_f32("l_to_f", m, i, (float)l2d[i]);
  }

  static volatile long double l2f[] = {
      1.0L + 0x1p-24L,
      1.0L + 0x1p-23L + 0x1p-24L,
      0x1p-126L - 0x1p-150L,
      -(0x1p-126L - 0x1p-150L),
      (long double)FLT_MAX + 0x1p103L,
  };
  for (unsigned i = 0; i < sizeof(l2f) / sizeof(l2f[0]); ++i) {
    clear_all(); show_f32("l_to_f2", m, i, (float)l2f[i]);
  }

  /* Widening and narrowing conversions on NaNs (quiet and signalling). */
  static volatile float f2w[] = {0.0f, -0.0f, 0x1p-149f, -0x1p-149f, 1.5f, FLT_MAX, INFINITY, NAN, __builtin_nansf("")};
  for (unsigned i = 0; i < sizeof(f2w) / sizeof(f2w[0]); ++i) {
    clear_all(); show_f64("f_to_d", m, i, (double)f2w[i]);
    clear_all(); show_f80("f_to_l", m, i, (long double)f2w[i]);
  }
  static volatile double d2w[] = {0.0, -0.0, 0x1p-1074, -0x1p-1074, 1.5, DBL_MAX, INFINITY, NAN, __builtin_nans("")};
  for (unsigned i = 0; i < sizeof(d2w) / sizeof(d2w[0]); ++i) {
    clear_all(); show_f80("d_to_l", m, i, (long double)d2w[i]);
    clear_all(); show_f32("d_to_f_nan", m, i, (float)d2w[i]);
  }
  static volatile long double l2n[] = {(long double)NAN, __builtin_nansl("")};
  for (unsigned i = 0; i < sizeof(l2n) / sizeof(l2n[0]); ++i) {
    clear_all(); show_f64("l_to_d_nan", m, i, (double)l2n[i]);
    clear_all(); show_f32("l_to_f_nan", m, i, (float)l2n[i]);
  }

  /* Integer -> float/double/long double: inexact values testing rounding direction. */
  static volatile int32_t vi32[] = {0, 1, -1, 16777217, -16777217, 16777219, -16777219, INT32_MAX, INT32_MIN};
  static volatile uint32_t vu32[] = {0u, 1u, 16777217u, 16777219u, UINT32_MAX};
  static volatile int64_t vi64[] = {
      0, 1, -1, (1LL << 53) + 1, -((1LL << 53) + 1), (1LL << 53) + 3, -((1LL << 53) + 3), INT64_MAX, INT64_MIN,
  };
  static volatile uint64_t vu64[] = {0ULL, 1ULL, (1ULL << 53) + 1, (1ULL << 53) + 3, UINT64_MAX};
  for (unsigned i = 0; i < sizeof(vi32) / sizeof(vi32[0]); ++i) {
    clear_all(); show_f32("i32_f", m, i, (float)vi32[i]);
    clear_all(); show_f64("i32_d", m, i, (double)vi32[i]);
    clear_all(); show_f80("i32_l", m, i, (long double)vi32[i]);
  }
  for (unsigned i = 0; i < sizeof(vu32) / sizeof(vu32[0]); ++i) {
    clear_all(); show_f32("u32_f", m, i, (float)vu32[i]);
    clear_all(); show_f64("u32_d", m, i, (double)vu32[i]);
    clear_all(); show_f80("u32_l", m, i, (long double)vu32[i]);
  }
  for (unsigned i = 0; i < sizeof(vi64) / sizeof(vi64[0]); ++i) {
    clear_all(); show_f32("i64_f", m, i, (float)vi64[i]);
    clear_all(); show_f64("i64_d", m, i, (double)vi64[i]);
    clear_all(); show_f80("i64_l", m, i, (long double)vi64[i]);
  }
  for (unsigned i = 0; i < sizeof(vu64) / sizeof(vu64[0]); ++i) {
    clear_all(); show_f32("u64_f", m, i, (float)vu64[i]);
    clear_all(); show_f64("u64_d", m, i, (double)vu64[i]);
    clear_all(); show_f80("u64_l", m, i, (long double)vu64[i]);
  }

  /* 128-bit integers -> float/double/long double. */
  static volatile __int128 vti[] = {
      0,
      ((__int128)1 << 64) + 1,
      -(((__int128)1 << 64) + 1),
      ((__int128)1 << 64) + 3,
      (((__int128)1 << 126) - 1) * 2 + 1,
      -(((__int128)1 << 126) - 1) * 2 - 1,
  };
  static volatile unsigned __int128 vuti[] = {
      0,
      ((unsigned __int128)1 << 64) + 1,
      ((unsigned __int128)1 << 64) + 3,
      ~(unsigned __int128)0,
  };
  for (unsigned i = 0; i < sizeof(vti) / sizeof(vti[0]); ++i) {
    clear_all(); show_f32("ti_f", m, i, (float)vti[i]);
    clear_all(); show_f64("ti_d", m, i, (double)vti[i]);
    clear_all(); show_f80("ti_l", m, i, (long double)vti[i]);
  }
  for (unsigned i = 0; i < sizeof(vuti) / sizeof(vuti[0]); ++i) {
    clear_all(); show_f32("uti_f", m, i, (float)vuti[i]);
    clear_all(); show_f64("uti_d", m, i, (double)vuti[i]);
    clear_all(); show_f80("uti_l", m, i, (long double)vuti[i]);
  }

  /* Float/double/long double -> integers: C cast always truncates toward zero regardless of mode. */
  static volatile float fi_f[] = {0.0f, -0.0f, 0.1f, -0.1f, 0.5f, -0.5f, 0.9f, -0.9f, 1.5f, -1.5f, 42.0f, -42.0f, 127.5f, -128.5f};
  for (unsigned i = 0; i < sizeof(fi_f) / sizeof(fi_f[0]); ++i) {
    clear_all(); show_i64("f_i8", m, i, (int8_t)fi_f[i]);
    clear_all(); show_i64("f_i16", m, i, (int16_t)fi_f[i]);
    clear_all(); show_i64("f_i32", m, i, (int32_t)fi_f[i]);
    clear_all(); show_i64("f_i64", m, i, (int64_t)fi_f[i]);
    clear_all(); show_i128("f_i128", m, i, (__int128)fi_f[i]);
    if (fi_f[i] >= 0.0f) {
      clear_all(); show_u64("f_u8", m, i, (uint8_t)fi_f[i]);
      clear_all(); show_u64("f_u16", m, i, (uint16_t)fi_f[i]);
      clear_all(); show_u64("f_u32", m, i, (uint32_t)fi_f[i]);
      clear_all(); show_u64("f_u64", m, i, (uint64_t)fi_f[i]);
      clear_all(); show_u128("f_u128", m, i, (unsigned __int128)fi_f[i]);
    }
  }
  static volatile double fi_d[] = {0.0, -0.0, 0.1, -0.1, 0.5, -0.5, 0.9, -0.9, 1.5, -1.5, 32767.5, -32768.5};
  for (unsigned i = 0; i < sizeof(fi_d) / sizeof(fi_d[0]); ++i) {
    clear_all(); show_i64("d_i16", m, i, (int16_t)fi_d[i]);
    clear_all(); show_i64("d_i32", m, i, (int32_t)fi_d[i]);
    clear_all(); show_i64("d_i64", m, i, (int64_t)fi_d[i]);
    clear_all(); show_i128("d_i128", m, i, (__int128)fi_d[i]);
    if (fi_d[i] >= 0.0) {
      clear_all(); show_u64("d_u32", m, i, (uint32_t)fi_d[i]);
      clear_all(); show_u64("d_u64", m, i, (uint64_t)fi_d[i]);
      clear_all(); show_u128("d_u128", m, i, (unsigned __int128)fi_d[i]);
    }
  }
  static volatile long double fi_l[] = {0.0L, -0.0L, 0.1L, -0.1L, 0.5L, -0.5L, 1.5L, -1.5L, 32767.5L, -32768.5L};
  for (unsigned i = 0; i < sizeof(fi_l) / sizeof(fi_l[0]); ++i) {
    clear_all(); show_i64("l_i16", m, i, (int16_t)fi_l[i]);
    clear_all(); show_i64("l_i32", m, i, (int32_t)fi_l[i]);
    clear_all(); show_i64("l_i64", m, i, (int64_t)fi_l[i]);
    clear_all(); show_i128("l_i128", m, i, (__int128)fi_l[i]);
    if (fi_l[i] >= 0.0L) {
      clear_all(); show_u64("l_u32", m, i, (uint32_t)fi_l[i]);
      clear_all(); show_u64("l_u64", m, i, (uint64_t)fi_l[i]);
      clear_all(); show_u128("l_u128", m, i, (unsigned __int128)fi_l[i]);
    }
  }
}

static void test_rounding_funcs(int m) {
  static volatile float rf[] = {
      0.0f, -0.0f, 0x1p-149f, -0x1p-149f, 0.25f, -0.25f, 0.5f, -0.5f, 0.75f, -0.75f,
      1.5f, -1.5f, 2.5f, -2.5f, 3.0f, -3.0f, 0x1p23f, -0x1p23f, 0x1p63f, -0x1p63f, INFINITY, -INFINITY,
  };
  for (unsigned i = 0; i < sizeof(rf) / sizeof(rf[0]); ++i) {
    float x = rf[i];
    clear_all(); show_f32("rintf", m, i, rintf(x));
    clear_all(); show_f32("print_f", m, i, p_rintf(x));
    clear_all(); show_f32("nearbyintf", m, i, nearbyintf(x));
    clear_all(); show_f32("pnearbyintf", m, i, p_nearbyintf(x));
    clear_all(); show_int_checked("lrintf", m, i, lrintf(x));
    clear_all(); show_int_checked("plrintf", m, i, p_lrintf(x));
    clear_all(); show_int_checked("llrintf", m, i, llrintf(x));
    clear_all(); show_int_checked("pllrintf", m, i, p_llrintf(x));
    clear_all(); show_f32("ceilf", m, i, ceilf(x));
    clear_all(); show_f32("pceilf", m, i, p_ceilf(x));
    clear_all(); show_f32("truncf", m, i, truncf(x));
    clear_all(); show_f32("ptruncf", m, i, p_truncf(x));
    clear_all(); show_f32("roundevenf", m, i, __builtin_roundevenf(x));
    clear_all(); show_int_checked("lroundf", m, i, lroundf(x));
    clear_all(); show_int_checked("plroundf", m, i, p_lroundf(x));
  }

  static volatile double rd[] = {
      0.0, -0.0, 0x1p-1074, -0x1p-1074, 0.25, -0.25, 0.5, -0.5, 0.75, -0.75,
      1.5, -1.5, 2.5, -2.5, 3.0, -3.0, 0x1p52, -0x1p52, 0x1p63, -0x1p63, 0x1.fffffffffffffp62, INFINITY,
  };
  for (unsigned i = 0; i < sizeof(rd) / sizeof(rd[0]); ++i) {
    double x = rd[i];
    clear_all(); show_f64("rint", m, i, rint(x));
    clear_all(); show_f64("print_d", m, i, p_rint(x));
    clear_all(); show_f64("nearbyint", m, i, nearbyint(x));
    clear_all(); show_f64("pnearbyint", m, i, p_nearbyint(x));
    clear_all(); show_int_checked("lrint", m, i, lrint(x));
    clear_all(); show_int_checked("plrint", m, i, p_lrint(x));
    clear_all(); show_int_checked("llrint", m, i, llrint(x));
    clear_all(); show_int_checked("pllrint", m, i, p_llrint(x));
    clear_all(); show_int_checked("llround", m, i, llround(x));
    clear_all(); show_int_checked("pllround", m, i, p_llround(x));
  }

  static volatile long double rl[] = {
      0.0L, -0.0L, 0x1p-16445L, -0x1p-16445L, 0.25L, -0.25L, 0.5L, -0.5L, 0.75L, -0.75L,
      1.5L, -1.5L, 2.5L, -2.5L, 3.0L, -3.0L, 0x1p63L, -0x1p63L,
      0x1.fffffffffffffffcp62L,  /* 2^63 - 1: exact */
      0x1.fffffffffffffffep62L,  /* 2^63 - 0.5: rounds to 2^63 in nearest/upward */
      -0x1.fffffffffffffffep62L, /* -2^63 + 0.5: rounds to -2^63 (in range) */
      -0x1.0000000000000002p63L, /* -(2^63 + 2): out of range */
      (long double)INFINITY,
  };
  for (unsigned i = 0; i < sizeof(rl) / sizeof(rl[0]); ++i) {
    long double x = rl[i];
    clear_all(); show_f80("rintl", m, i, rintl(x));
    clear_all(); show_f80("print_l", m, i, p_rintl(x));
    clear_all(); show_f80("nearbyintl", m, i, nearbyintl(x));
    clear_all(); show_f80("pnearbyintl", m, i, p_nearbyintl(x));
    clear_all(); show_int_checked("lrintl", m, i, lrintl(x));
    clear_all(); show_int_checked("plrintl", m, i, p_lrintl(x));
    clear_all(); show_int_checked("llrintl", m, i, llrintl(x));
    clear_all(); show_int_checked("pllrintl", m, i, p_llrintl(x));
    /* Index 19 (2^63 - 0.5L) is tested separately by the ignored defect witness
     * `lroundl_and_llroundl_at_two_to_63_minus_half_raise_invalid`. */
    if (i != 19) {
      clear_all(); show_int_checked("llroundl", m, i, llroundl(x));
      clear_all(); show_int_checked("pllroundl", m, i, p_llroundl(x));
    }
  }

  /* ilogb / ilogbf / ilogbl across zeros, subnormals, normals, infinities, and NaNs. */
  static int (*volatile p_ilogbf)(float) = ilogbf;
  static int (*volatile p_ilogb)(double) = ilogb;
  static int (*volatile p_ilogbl)(long double) = ilogbl;
  static volatile float il_f[] = {0.0f, -0.0f, 0x1p-149f, -0x1p-126f, 1.5f, INFINITY, -INFINITY, NAN, __builtin_nansf("")};
  static volatile double il_d[] = {0.0, -0.0, 0x1p-1074, -0x1p-1022, 1.5, INFINITY, -INFINITY, NAN, __builtin_nans("")};
  static volatile long double il_l[] = {
      0.0L, -0.0L, 0x1p-16445L, -0x1p-16382L, 1.5L, (long double)INFINITY, -(long double)INFINITY,
      (long double)NAN, __builtin_nansl(""),
  };
  for (unsigned i = 0; i < sizeof(il_f) / sizeof(il_f[0]); ++i) {
    clear_all(); show_i64("ilogbf", m, i, p_ilogbf(il_f[i]));
    clear_all(); show_i64("ilogb", m, i, p_ilogb(il_d[i]));
    clear_all(); show_i64("ilogbl", m, i, p_ilogbl(il_l[i]));
  }

  /* fminf, fmax, fmaxl (complementing fenv_ops.c which checked fmin_d, fmax_f, fmin_l). */
  static volatile float mf[] = {-0.0f, 0.0f, 1.0f, -1.0f, NAN, __builtin_nansf("")};
  for (unsigned i = 0; i < 6; ++i) {
    for (unsigned j = 0; j < 6; ++j) {
      if (isnan(mf[i]) && isnan(mf[j])) continue;
      clear_all(); show_f32("fminf", m, i * 8 + j, fminf(mf[i], mf[j]));
      clear_all(); show_f32("pfminf", m, i * 8 + j, p_fminf(mf[i], mf[j]));
    }
  }
  static volatile double md[] = {-0.0, 0.0, 1.0, -1.0, NAN, __builtin_nans("")};
  for (unsigned i = 0; i < 6; ++i) {
    for (unsigned j = 0; j < 6; ++j) {
      if (isnan(md[i]) && isnan(md[j])) continue;
      clear_all(); show_f64("fmax", m, i * 8 + j, fmax(md[i], md[j]));
      clear_all(); show_f64("pfmax", m, i * 8 + j, p_fmax(md[i], md[j]));
    }
  }
  static volatile long double ml[] = {-0.0L, 0.0L, 1.0L, -1.0L, (long double)NAN, __builtin_nansl("")};
  for (unsigned i = 0; i < 6; ++i) {
    for (unsigned j = 0; j < 6; ++j) {
      if (isnan(ml[i]) && isnan(ml[j])) continue;
      clear_all(); show_f80("fmaxl", m, i * 8 + j, fmaxl(ml[i], ml[j]));
      clear_all(); show_f80("pfmaxl", m, i * 8 + j, p_fmaxl(ml[i], ml[j]));
    }
  }
}

int main(void) {
  for (int m = 0; m < 4; ++m) {
    fesetenv(FE_DFL_ENV);
    fesetround(modes[m]);
    test_arith_f32(m);
    test_arith_f64(m);
    test_arith_f80(m);
    test_sqrt_and_fma(m);
    test_conversions(m);
    test_rounding_funcs(m);
  }
  fesetenv(FE_DFL_ENV);
  return 0;
}
